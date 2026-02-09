# Freecut

Freecut is a Rust service for 2D rectangular cut optimization with an HTTP API and optional SVG output.
It uses Axum and the `cut-optimizer-2d` engine.

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

## Main Endpoints
- `POST /v1/optimize` - standard optimizer (with optional portfolio mode).
- `POST /v1/optimize/beam` - beam-search orchestration endpoint.
- `POST /v1/optimize/alns` - ALNS/LNS orchestration endpoint.

- Request/response are JSON.
- All dimensions are in millimeters (`mm`).
- Successful responses include SVG in `artifacts.svg` by default (`params.include_svg=true`).
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
    The service applies budget-aware restart slicing with a technical minimum slice of ~80 ms
    and a higher effective slice per SLA profile. Recommended starting range: 1000–2000 ms
    for typical cases, higher for large/complex inputs. Service default: `DEFAULT_TIME_LIMIT_MS=2000`.
  - `restarts`: Number of optimization restarts (multi-start). Optional in the request.
    Service default: `DEFAULT_RESTARTS=10`.
  - `sla_profile`: Optional restart-budget profile for `/v1/optimize`:
    - `"fast"`: fewer, longer attempts.
    - `"balanced"`: default compromise of speed and stability.
    - `"quality"`: allows more restarts with shorter effective slices.
  - `objective`: Optimization goal: `"min_waste"` or `"min_sheets"`.
    With identical stock sizes, both goals typically yield the same number of sheets;
    differences matter when multiple stock sizes are provided.
  - `seed`: Optional deterministic seed for reproducible results. If omitted, the
    service generates a seed per request (Unix epoch in ms) and returns it as `used_seed`.
  - `layout_mode`: Layout mode: `"guillotine"` (default, guillotine-only cuts) or `"nested"`. Optional in the request.
  - `placement_heuristic`: Optional placement heuristic preset. If omitted, the engine uses its default heuristic mix.
    - For `layout_mode="guillotine"`: `"best_area"`, `"best_short_side"`, `"best_long_side"`,
      `"worst_area"`, `"worst_short_side"`, `"worst_long_side"`, `"smallest_y"`.
    - For `layout_mode="nested"`: `"best_area"`, `"best_short_side"`, `"best_long_side"`,
      `"bottom_left"`, `"contact_point"`.
    - Portfolio mode will rotate heuristics across candidates when this value is omitted.
  - `placement_bias`: Optional placement bias weights to reduce edge-hugging in layout.
  - `placement_bias.edge_penalty`: Penalty for placements near sheet edges. Optional.
  - `placement_bias.center_pull`: Pull toward sheet center. Optional.
  - `placement_bias.bbox_weight`: Penalty for expanding the occupied bounding box. Optional.
  - `placement_bias.fragmentation_penalty`: Penalty for creating thin leftover slivers in the free rectangle. Optional.
  - `placement_bias.tie_break_jitter`: Deterministic jitter to break ties when placement scores are equal. Optional.
  - `fitness_weights`: Optional composite fitness weights for internal GA scoring. Omit to keep legacy waste-only scoring.
  - `fitness_weights.waste`: Weight for waste minimization (legacy fitness). Optional, defaults to 1.0.
  - `fitness_weights.void`: Weight for internal void reduction (bbox void area). Optional.
  - `fitness_weights.compactness`: Weight for compactness (used_area / bbox_area). Optional.
  - `fitness_weights.perimeter`: Weight for perimeter compactness (4*sqrt(area) / perimeter). Optional.
  - `include_svg`: Optional flag (`true` by default). Set to `false` to skip SVG generation and omit `artifacts.svg` in response.
  - `portfolio`: Optional anytime orchestration settings.
    - `enabled`: Optional (`true` by default when object is present).
    - `deadline_ms`: Optional total portfolio deadline; defaults to `time_limit_ms`.
    - `candidate_count`: Optional number of portfolio candidates (`1..16`, default `4`).
  - `beam`: Optional beam-search settings (primarily for `/v1/optimize/beam`).
    - `enabled`: Optional (`true` by default when object is present).
    - `deadline_ms`: Optional total beam deadline; defaults to `time_limit_ms`.
    - `beam_width`: Optional beam width (`1..8`, default `2`).
    - `beam_depth`: Optional beam depth (`1..8`, default `2`).
    - `branch_factor`: Optional branch factor (`1..8`, default `2`).
  - `alns`: Optional ALNS/LNS settings (primarily for `/v1/optimize/alns`).
    - `enabled`: Optional (`true` by default when object is present).
    - `deadline_ms`: Optional ALNS deadline; defaults to `time_limit_ms`.
    - `iterations`: Optional iteration count (`1..512`, default `24`).
    - `segment_size`: Optional adaptive update cadence (`1..64`, default `6`).
    - `temperature_start`: Optional start temperature (`>0`, default `1.0`).
    - `temperature_end`: Optional end temperature (`>0`, `<= temperature_start`, default `0.12`).
    - `reaction_factor`: Optional adaptive reaction (`(0,1]`, default `0.3`).
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
  - `restarts_requested`: Restarts requested by client/default before internal slicing/budget adjustments.
  - `used_seed`: Seed actually used (user-provided or auto-generated).
  - `layout_mode`: Layout mode actually used.
  - `timeout_reason`: Optional; appears when optimization stopped early due budget (`"slice_timeout"` or `"time_budget_exhausted"`).
  - `restart_policy`: Optional telemetry for restart budgeting in `/v1/optimize`:
    - `profile`, `min_slice_ms`, `min_effective_slice_ms`,
      `restarts_cap_by_effective_slice`, `restarts_effective`,
      `baseline_budget_ms`, `progressive_slicing`, `planned_slices_ms`,
      `timeouts_per_restart`, `first_timeout_at_restart`, `best_found_at_restart`,
      `rescue_used`, `rescue_budget_ms`.
  - `portfolio`: Optional telemetry for portfolio mode:
    - `deadline_ms`, `candidates_total`, `candidates_completed`, `candidates_timed_out`,
      `candidates_failed`, `candidates_skipped`, `winner_strategy`, `winner_seed`,
      `winner_restarts_used`.
  - `beam`: Optional telemetry for beam mode:
    - `deadline_ms`, `beam_width`, `beam_depth`, `branch_factor`,
      `nodes_evaluated`, `nodes_timed_out`, `nodes_failed`, `nodes_pruned`,
      `winner_depth`, `winner_seed`, `winner_restarts_used`.
  - `alns`: Optional telemetry for ALNS/LNS mode:
    - `deadline_ms`, `iterations_requested`, `iterations_completed`, `segment_size`,
      `temperature_start`, `temperature_end`, `reaction_factor`,
      `candidates_evaluated`, `candidates_timed_out`, `candidates_failed`,
      `accepted_worse`, `improved_best`, `winner_seed`, `winner_restarts_used`.
    - `operators`: adaptive operator stats (`name`, `weight`, `selected`, `accepted`, `improved_best`).
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
- `artifacts.svg`: Optional. Present when `params.include_svg=true` (default). Multiple sheets are rendered vertically with 50mm gap between them.

## Quality Gate (Top-20 to Top-10)
For this service's business context, metric-only ranking is not sufficient. Visual validation is mandatory because formulas do not capture all practical cutting nuances.

1. Auto-filter to `Top-20`:
- Drop candidates where placeable parts are not fully placed (`placeable_placed_ratio < 1.0`).
- Drop candidates with internal holes (`internal_void > 0`).
- Rank by: `internal_void -> occupied_perimeter -> void_compactness -> corridor_components -> waste_percent`.
- Remove near-duplicate layouts to preserve diversity.

2. Visual review `Top-20 -> Top-10`:
- Check corridor/void geometry in SVG.
- Check cut-path practicality and sheet usage logic.
- Reject layouts with "artificially good" waste caused by under-placement.
- Keep final `Top-10` with metrics + SVG artifacts for auditability.

Example candidate generation command:
```bash
FREECUT_NUM_TESTS=500 FREECUT_TOP_N=20 python3 scripts/optimize_search.py
```

## Environment Variables
- `PORT` (default `8088`)
- `RUST_LOG` (default `info`)
- `MAX_BODY_BYTES` (default `5242880`)
- `MAX_INSTANCES` (default `5000`)
- `DEFAULT_TIME_LIMIT_MS` (default `2000`)
- `DEFAULT_RESTARTS` (default `10`)
- `MAX_CONCURRENT_OPTIMIZE` (default: available CPU count, minimum `1`)

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

## TODO
- Fix `stock_map` key collision for same usable sheet size:
  - Current risk: mapping by `(usable_w, usable_h)` can merge different stock entries with different `stock_id`/`qty`.
  - Impact: wrong `stock_id` in response and incorrect `qty_limit` behavior (false `unplaced_items` with `reason="qty_limit"`).
  - Business context: this fix is required primarily for business correctness (material identity, stock accounting, order semantics), even when geometric placement itself looks valid.
  - Required change: use a stable per-stock key (or preserve source stock identity through optimization mapping), not only size tuple.
- Stabilize timeout-rescue regression tests and document production tuning:
  - Tests `optimize_multisheet_restarts_4_*` can intermittently fail with `408 TIMEOUT` under parallel `cargo test` due to CPU contention.
  - Mitigations: serialize CPU-heavy optimize tests (global semaphore/lock) or run CI with `RUST_TEST_THREADS=1`.
  - Production: set `MAX_CONCURRENT_OPTIMIZE` to match real CPU quota to prefer `429 OVERLOADED` over avoidable `408 TIMEOUT`.

## License
This project is licensed under the **MIT License**. See `LICENSE`.

## Notes
- Pattern direction flags are validated for rotation constraints, but only `none` currently affects optimization.
