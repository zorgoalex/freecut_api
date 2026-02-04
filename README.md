# Freecut

Freecut is a Rust service for 2D rectangular cut optimization with an HTTP API and SVG output.
It uses Axum and the `cut-optimizer-2d` engine and always returns an SVG artifact for successful optimizations.

## Features
- 2D rectangle nesting with kerf/spacing/trim support
- Rotation constraints and pattern direction flags
- Multi-start optimization with deterministic seeds
- **Multi-sheet support** with automatic sheet allocation
- **Partial placement** with unplaced items tracking
- **SVG visualization** of multiple sheets (vertical layout)
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

Service listens on `0.0.0.0:8088` by default.

## Quick Start (Docker)
```bash
docker build -t freecut-mvp .
docker run --rm -p 8088:8088 freecut-mvp
```

## Health & Docs
- `GET /health/live`
- `GET /health/ready`
- `GET /version`
- `GET /openapi.json`
- `GET /docs`
 - Generated schema file: `openapi.json`

## Main Endpoint
`POST /v1/optimize`

- Request/response are JSON.
- All dimensions are in millimeters (`mm`).
- Successful responses include SVG in `artifacts.svg`.
 - Coordinate system: origin (0,0) is the **top-left** of the usable area (after `trim_mm`), X to the right, Y down.

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
    "seed": 12345,
    "layout_mode": "nested"
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
  - `spacing_mm`: Additional clearance between parts, added on top of `kerf_mm`.
    The effective gap between parts is `kerf_mm + spacing_mm`.
  - `trim_mm`: Unusable margins around the sheet in mm.
    - `left`, `right`, `top`, `bottom`: Margin sizes in mm.
  - `time_limit_ms`: Total time budget for optimization in milliseconds. Optional in the request.
    The time is split across restarts; if the per-restart slice drops below ~80 ms,
    the service reduces the actual number of restarts. Recommended starting range:
    1000–2000 ms for typical cases, higher for large/complex inputs. Service default:
    `DEFAULT_TIME_LIMIT_MS=2000`.
  - `restarts`: Number of optimization restarts (multi-start). Optional in the request.
    Service default: `DEFAULT_RESTARTS=10`.
  - `objective`: Optimization goal: `"min_waste"` or `"min_sheets"`.
    With identical stock sizes, both goals typically yield the same number of sheets;
    differences matter when multiple stock sizes are provided.
  - `seed`: Optional deterministic seed for reproducible results. If omitted, the
    service generates a seed per request (Unix epoch in ms) and returns it as `used_seed`.
  - `layout_mode`: Layout mode: `"guillotine"` (default, guillotine-only cuts) or `"nested"`. Optional in the request.
- `stock`: Available sheet materials.
  - `id`: Stock identifier (your business label for a sheet type).
  - `width_mm`, `height_mm`: Sheet dimensions in mm.
  - `qty`: Quantity of sheets of this size. **Optional**: if omitted or `0`, unlimited sheets will be used automatically.
- `items`: Parts to be cut.
  - `id`: Part identifier (your business label).
  - `width_mm`, `height_mm`: Part dimensions in mm.
  - `qty`: Quantity of this part.
  - `rotation`: Rotation rule: `"forbid"` or `"allow_90"`.
  - `pattern_direction`: Grain/pattern direction: `"none"`, `"along_width"`, `"along_height"`.

### Example Response
Example file: `examples/optimize_response_ok.json`

```json
{
  "status": "ok",
  "summary": {
    "objective": "min_waste",
    "used_stock_count": 1,
    "total_waste_area_mm2": 680400.0,
    "waste_percent": 70.8455,
    "time_ms": 3,
    "restarts_used": 3,
    "used_seed": 12345,
    "layout_mode": "nested"
  },
  "solutions": [
    {
      "stock_id": "sheet-1000",
      "index": 0,
      "width_mm": 1000.0,
      "height_mm": 1000.0,
      "trim_mm": { "left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0 },
      "placements": [
        {
          "item_id": "B",
          "instance": 1,
          "x_mm": 0.0,
          "y_mm": 0.0,
          "width_mm": 400.0,
          "height_mm": 400.0,
          "rotated": false,
          "pattern_direction": "none"
        }
      ]
    }
  ],
  "artifacts": {
    "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-10 -10 1000 1000\">...</svg>"
  }
}
```

### Response Keys (Summary)
- `status`: `"ok"` on success.
- `summary`: Aggregated optimization metrics.
  - `objective`: Chosen objective (`"min_waste"` or `"min_sheets"`).
  - `used_stock_count`: Number of sheets used.
  - `total_waste_area_mm2`: Total waste area in mm².
  - `waste_percent`: Waste percentage of used stock.
  - `time_ms`: Total runtime in milliseconds.
  - `restarts_used`: Number of restarts actually used.
- `used_seed`: Seed actually used (user-provided or auto-generated).
  - `layout_mode`: Layout mode actually used.
- `solutions`: Per-sheet layouts.
  - `stock_id`: Stock ID from request.
  - `index`: Sheet index for that stock type.
  - `width_mm`, `height_mm`: Sheet dimensions.
  - `trim_mm`: Margins used.
  - `placements`: List of placed parts.
    - `item_id`: Item ID from request.
    - `instance`: Instance number.
    - `x_mm`, `y_mm`, `width_mm`, `height_mm`: Placement geometry.
    - `rotated`: Whether part was rotated.
    - `pattern_direction`: Direction from request.
- `unplaced_items`: (Optional) Items that could not be placed. Present when items are oversized or when `stock.qty` limits are exceeded.
  - `item_id`: Item ID from request.
  - `instance`: Instance number.
  - `width_mm`, `height_mm`: Item dimensions.
  - `reason`: Why the item was not placed:
    - `"oversized"` — item dimensions exceed usable sheet area (after trim and gap).
    - `"qty_limit"` — item fits but no sheets available due to `stock.qty` limit.
- `artifacts.svg`: Full SVG document of the layout. Multiple sheets are rendered vertically with 50mm gap between them.

## Environment Variables
- `PORT` (default `8088`)
- `RUST_LOG` (default `info`)
- `MAX_BODY_BYTES` (default `5242880`)
- `MAX_INSTANCES` (default `5000`)
- `DEFAULT_TIME_LIMIT_MS` (default `2000`)
- `DEFAULT_RESTARTS` (default `10`)

## Testing
```bash
cargo test
```

Note: Swagger UI assets are downloaded during build; tests/builds may require network access.

## Docker Smoke Tests
These tests validate the running container via a host-network curl image.

```bash
# Start the container first
docker run --rm -p 8088:8088 freecut-mvp

# In another terminal
./scripts/docker_smoke.sh
```

Optional overrides:
```bash
BASE_URL=http://127.0.0.1:8088 CURL_IMAGE=curlimages/curl:8.6.0 ./scripts/docker_smoke.sh
```

## Greedy Multi-Sheet Optimizer

For complex multi-sheet layouts, use the greedy optimizer script that samples random item combinations:

```bash
python3 scripts/greedy_optimize.py -i request.json [-t 60]
```

**Features:**
- Tries multiple first-sheet combinations
- Evaluates full sequence to minimize overall waste
- Time limit via CLI (`-t 60`) or JSON (`params.search_time_limit_ms: 60000`)
- Default: 25 seconds, max recommended: 5 minutes
- Early stopping when no improvement found

**Algorithm:**
1. Sample random subsets of items that fit on one sheet
2. For each subset, evaluate the full multi-sheet sequence
3. Pick the combination with lowest overall waste
4. Repeat with remaining items for subsequent sheets

## License
This project is licensed under the **MIT License**. See `LICENSE`.

## Notes
- Pattern direction flags are validated for rotation constraints, but only `none` currently affects optimization.
