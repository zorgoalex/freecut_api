# Freecut API Specification

This document describes the current Freecut HTTP API.

Freecut is a service for 2D rectangular sheet-cut optimization. It accepts stock sheets, rectangular parts, and cutting parameters, then returns sheet layouts, placement coordinates, optimization telemetry, and optional SVG visualization.

All dimensions are provided and returned in millimeters.

## Base URL

Default local URL:

```text
http://localhost:8088
```

The port is configured with the `PORT` environment variable.

## Service Endpoints

### GET /health/live

Liveness probe.

Response `200 text/plain`:

```text
ok
```

### GET /health/ready

Readiness probe.

Response `200 text/plain`:

```text
ok
```

### GET /version

Service version.

Response `200 application/json`:

```json
{
  "service": "freecut",
  "version": "0.1.0"
}
```

### GET /docs

Swagger UI.

### GET /openapi.json

OpenAPI JSON schema.

## Optimization Endpoints

All optimization endpoints accept the same `OptimizeRequest` JSON payload.

### POST /v1/optimize

Main optimization endpoint.

It supports:

- `layout_mode: "guillotine"`, `"nested"`, or `"vacuum_table"`;
- multi-start GA optimization;
- optional `profile_pool`;
- optional `partition`;
- optional `group_shift`;
- optional smart retry.

### POST /v1/optimize/beam

Beam-search orchestration endpoint. It uses the same request schema. Beam settings are passed in `params.beam`.

### POST /v1/optimize/alns

ALNS/LNS orchestration endpoint. It uses the same request schema. ALNS settings are passed in `params.alns`.

## Headers

Request:

```http
Content-Type: application/json
```

Response:

```http
Content-Type: application/json
```

## OptimizeRequest

Basic request shape:

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
    "time_limit_ms": 2000,
    "restarts": 10,
    "objective": "min_waste",
    "seed": 12345,
    "layout_mode": "guillotine",
    "include_svg": true
  },
  "stock": [
    {
      "id": "sheet-2800x2070",
      "width_mm": 2070.0,
      "height_mm": 2800.0,
      "qty": 0
    }
  ],
  "items": [
    {
      "id": "part-a",
      "width_mm": 700.0,
      "height_mm": 980.0,
      "qty": 2,
      "rotation": "allow_90",
      "pattern_direction": "none"
    }
  ]
}
```

### Top-Level Fields

| Field | Type | Required | Description |
|---|---:|---:|---|
| `units` | string | yes | Only `"mm"` is supported. |
| `params` | object | yes | Optimization parameters. |
| `stock` | array | yes | Available stock sheet types. Must contain at least one entry. |
| `items` | array | yes | Parts to place. Must contain at least one entry. |

## params

### Required params

| Field | Type | Required | Valid values | Description |
|---|---:|---:|---|---|
| `kerf_mm` | number | yes | `>= 0` | Physical cutting width / tool width. |
| `spacing_mm` | number | yes | `>= 0` | Additional clearance beyond kerf. |
| `trim_mm` | object | yes | all values `>= 0` | Unusable sheet margins. |
| `objective` | string | yes | `"min_waste"`, `"min_sheets"` | Optimization objective. |

The effective spacing between neighboring parts is:

```text
effective_gap_mm = kerf_mm + spacing_mm
```

### Optional params

| Field | Type | Default | Valid values | Description |
|---|---:|---:|---|---|
| `time_limit_ms` | integer | `DEFAULT_TIME_LIMIT_MS`, default env value `2000` | `>= 100` | Total optimization budget. |
| `restarts` | integer | `DEFAULT_RESTARTS`, default env value `10` | `>= 1` | Requested multi-start restarts. The service may reduce effective restarts based on budget. |
| `seed` | integer | generated from current time | `u64` | Deterministic seed for reproducible layouts. |
| `layout_mode` | string | `"guillotine"` | `"guillotine"`, `"nested"`, `"vacuum_table"` | Layout/cutting mode. |
| `vacuum` | object | omitted | see below | Vacuum-table profile settings; used only with `layout_mode: "vacuum_table"`. |
| `sla_profile` | string | `"balanced"` | `"fast"`, `"balanced"`, `"quality"` | Restart-budget profile for `/v1/optimize`. |
| `ga_profile` | string | `"balanced"` | `"fast"`, `"balanced"`, `"quality"` | GA internal profile. |
| `include_svg` | boolean | `true` | `true`, `false` | Include `artifacts.svg` in the response. |
| `retry_strategy` | string | `"smart"` | `"disabled"`, `"smart"` | Fault-aware retry behavior. |
| `max_retry_attempts` | integer | `3` | `>= 1`; values below 1 are clamped to 1 | Total attempts including the first attempt when retry is smart. |
| `ga_override` | object | omitted | see below | Advanced GA tuning. |
| `profile_pool` | object | omitted | see below | Multi-profile zone/remnant-aware selection. |
| `portfolio` | object | omitted | see below | Portfolio orchestration. |
| `beam` | object | omitted | see below | Beam-search orchestration. |
| `alns` | object | omitted | see below | ALNS/LNS orchestration. |
| `partition` | object | omitted | see below | Dense-first peeling/partition mode. |
| `group_shift` | object | omitted | see below | Post-process group compaction. |

## Kerf vs Spacing

`kerf_mm` is the physical width of the cut. It is the material removed by the tool: saw blade, router bit, laser beam, plasma cut, etc.

`spacing_mm` is extra process clearance beyond the tool width. It can represent safety clearance, tabs, clamping tolerance, vibration allowance, or edge-quality margin.

Example:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 1.0
}
```

The effective gap between adjacent parts is `7.0 mm`.

## trim_mm

```json
{
  "left": 10.0,
  "right": 10.0,
  "top": 10.0,
  "bottom": 10.0
}
```

All values must be `>= 0`.

For every stock sheet:

```text
usable_width = stock.width_mm - trim_mm.left - trim_mm.right
usable_height = stock.height_mm - trim_mm.top - trim_mm.bottom
```

Both usable dimensions must be `> 0`.

## stock[]

```json
{
  "id": "sheet-2800x2070",
  "width_mm": 2070.0,
  "height_mm": 2800.0,
  "qty": 20
}
```

| Field | Type | Required | Valid values | Description |
|---|---:|---:|---|---|
| `id` | string | yes | unique inside `stock` | Business id / material id. |
| `width_mm` | number | yes | `> 0` | Full sheet width. |
| `height_mm` | number | yes | `> 0` | Full sheet height. |
| `qty` | integer/null | no | `>= 0` | Available sheet quantity. If omitted or `0`, stock is treated as unlimited. |

`stock` limits:

- must not be empty;
- ids must be unique;
- maximum stock types: `50`.

## items[]

```json
{
  "id": "side-panel",
  "width_mm": 500.0,
  "height_mm": 800.0,
  "qty": 4,
  "rotation": "allow_90",
  "pattern_direction": "none"
}
```

| Field | Type | Required | Valid values | Description |
|---|---:|---:|---|---|
| `id` | string | yes | any string | Business id of the part. |
| `width_mm` | number | yes | `> 0` | Part width. |
| `height_mm` | number | yes | `> 0` | Part height. |
| `qty` | integer | yes | `>= 1` | Quantity of this part. |
| `rotation` | string | yes | `"forbid"`, `"allow_90"` | Whether 90-degree rotation is allowed. |
| `pattern_direction` | string | yes | `"none"`, `"along_width"`, `"along_height"` | Grain/pattern direction. |

Total item instances default limit is `MAX_INSTANCES=5000`.

Oversized parts are not rejected at request-validation level. They may be returned in `unplaced_items`.

## Enums

### objective

Values:

- `"min_waste"`: minimize total waste area.
- `"min_sheets"`: minimize number of sheets first.

### layout_mode

Values:

- `"guillotine"`: guillotine-style cuts.
- `"nested"`: nested rectangular placement mode.
- `"vacuum_table"`: single-stock vacuum-table profile. It builds a compact left/top-anchored cluster while preserving the effective kerf gap. It bypasses GA/restarts/profile-pool scoring and is intended for SketchCut-style vacuum press/table jobs.

## vacuum

Used only when `params.layout_mode = "vacuum_table"`.

```json
{
  "direction": "optimal"
}
```

| Field | Type | Default | Valid values | Description |
|---|---:|---:|---|---|
| `direction` | string | `"optimal"` | `"optimal"`, `"width"`, `"height"` | Row direction for the vacuum table. `optimal` evaluates both width-wise rows and height-wise columns. |

Vacuum-table mode requires exactly one `stock` entry. `stock.qty` limits how many table loads may be emitted; omitted or `0` means unlimited table loads. `kerf_mm + spacing_mm` is treated as the minimum clearance between neighboring parts. Remaining slack is pushed to the right/bottom side instead of being distributed as internal corridors.

Example vacuum-table request:

```json
{
  "units": "mm",
  "stock": [
    { "id": "vacuum_2800x1050", "width_mm": 2800.0, "height_mm": 1050.0, "qty": 1 }
  ],
  "items": [
    { "id": "mdf", "width_mm": 600.0, "height_mm": 300.0, "qty": 11, "rotation": "allow_90", "pattern_direction": "none" }
  ],
  "params": {
    "kerf_mm": 80.0,
    "spacing_mm": 0.0,
    "trim_mm": { "left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0 },
    "objective": "min_waste",
    "layout_mode": "vacuum_table",
    "vacuum": { "direction": "optimal" },
    "include_svg": true
  }
}
```

### rotation

Values:

- `"forbid"`
- `"allow_90"`

### pattern_direction

Values:

- `"none"`
- `"along_width"`
- `"along_height"`

### sla_profile

Values:

- `"fast"`: fewer/longer attempts.
- `"balanced"`: default compromise.
- `"quality"`: allows more search diversity.

### ga_profile

Values:

- `"fast"`
- `"balanced"`
- `"quality"`

## ga_override

Advanced GA tuning.

```json
{
  "epochs": 100,
  "breed_factor": 0.5,
  "survival_factor": 0.6,
  "top_k_candidates": 6,
  "zone_penalty": 0.3,
  "fill_penalty": 0.1
}
```

| Field | Type | Valid values | Description |
|---|---:|---|---|
| `epochs` | integer | `1..=2000` | Number of GA epochs. |
| `breed_factor` | number | finite, `(0, 1]` | Breed factor. |
| `survival_factor` | number | finite, `[0, 1]` | Survival factor. |
| `top_k_candidates` | integer | `1..=64` | Candidate pool size for business scorer. |
| `zone_penalty` | number | finite, `[0, 1]` | Waste-region penalty for GA fitness. |
| `fill_penalty` | number | finite, `[0, 1]` | Largest-waste-component fill penalty. |

## profile_pool

Multi-profile orchestration. It evaluates multiple `zone_penalty` profiles and selects a winner by sheet count, visual waste regions, cut-gap waste regions, lead utilization, and group-shift telemetry.

Research-quality example:

```json
{
  "enabled": true,
  "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8],
  "fill_penalty": 0.1,
  "max_lead_drop_pp": 0.8,
  "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21],
  "rescue_when_zones_gt": 5
}
```

Fields:

| Field | Type | Default | Valid values | Description |
|---|---:|---:|---|---|
| `enabled` | boolean | `true` when object is present | boolean | Enable profile pool. |
| `preset` | string | omitted | `"cheap"`, `"balanced_quality"`, `"aggressive"` | Named preset. Explicit fields override preset values. |
| `zone_penalties` | number[] | `[0.2, 0.3, 0.4, 0.5]` or preset default | length `1..=8`, values `[0,1]` | Main zone-penalty profiles. |
| `rescue_zone_penalties` | number[] | preset-dependent / empty | length `1..=8`, values `[0,1]` | Extra profiles for rescue stage. |
| `fill_penalty` | number | `ga_override.fill_penalty` or `0.1` | `[0,1]` | Fill penalty shared by profiles. |
| `max_lead_drop_pp` | number | `0.8` | `[0,10]` | Max allowed lead-utilization drop in winner selection. |
| `seed_offsets` | integer[] | empty | length `1..=8`, positive values | Extra seeds for adaptive rescue. |
| `rescue_when_zones_gt` | integer | `5` when seed offsets exist | `u32` | Trigger rescue if provisional winner has too many waste regions. |
| `rescue_when_max_corner_below_mm2` | number | omitted | finite, `>= 0` | Trigger rescue if largest reusable corner is too small. |
| `rescue_accept_min_max_corner_mm2` | number | omitted | finite, `>= 0` | Reject rescue candidates with too-small reusable corner. |

Preset values:

- `"cheap"`: smaller, cheaper profile pool.
- `"balanced_quality"`: balanced delayed rescue with reusable-corner guard.
- `"aggressive"`: always evaluates a broader pool.

## group_shift

Post-process compaction that shifts peripheral side groups toward a denser anchor cluster after optimization.

```json
{
  "enabled": true,
  "debug_artifacts": true,
  "min_shift_mm": 5.0,
  "max_passes": 4
}
```

Fields:

| Field | Type | Default | Valid values | Description |
|---|---:|---:|---|---|
| `enabled` | boolean | `true` when object is present | boolean | Enable group-shift postprocess. |
| `debug_artifacts` | boolean | `false` | boolean | Include before/diff SVG artifacts. |
| `min_shift_mm` | number | `5.0` | finite, `>= 0` | Ignore smaller moves. |
| `max_passes` | integer | `4` | `1..=16` | Maximum accepted shifts. |

When enabled, telemetry appears in `summary.group_shift`.

## partition

Dense-first peeling/partition mode.

```json
{
  "enabled": true,
  "sheet_budget_ms": 1000
}
```

| Field | Type | Default | Description |
|---|---:|---:|---|
| `enabled` | boolean | `true` when object is present | Enable partition mode. |
| `sheet_budget_ms` | integer | `time_limit_ms / planned_sheet_count` | Budget per peeling iteration. |

## portfolio

Anytime portfolio orchestration.

```json
{
  "enabled": true,
  "deadline_ms": 4000,
  "candidate_count": 4
}
```

| Field | Type | Default | Valid values |
|---|---:|---:|---|
| `enabled` | boolean | `true` when object is present | boolean |
| `deadline_ms` | integer | `time_limit_ms` | `>= 100` |
| `candidate_count` | integer | `4` | `1..=16` |

## beam

Beam search settings, primarily for `POST /v1/optimize/beam`.

```json
{
  "enabled": true,
  "deadline_ms": 4000,
  "beam_width": 2,
  "beam_depth": 2,
  "branch_factor": 2
}
```

| Field | Type | Default | Valid values |
|---|---:|---:|---|
| `enabled` | boolean | `true` when object is present | boolean |
| `deadline_ms` | integer | `time_limit_ms` | `>= 100` |
| `beam_width` | integer | `2` | `1..=8` |
| `beam_depth` | integer | `2` | `1..=8` |
| `branch_factor` | integer | `2` | `1..=8` |

## alns

ALNS/LNS settings, primarily for `POST /v1/optimize/alns`.

```json
{
  "enabled": true,
  "deadline_ms": 6000,
  "iterations": 24,
  "segment_size": 6,
  "temperature_start": 1.0,
  "temperature_end": 0.12,
  "reaction_factor": 0.3
}
```

| Field | Type | Default | Valid values |
|---|---:|---:|---|
| `enabled` | boolean | `true` when object is present | boolean |
| `deadline_ms` | integer | `time_limit_ms` | `>= 100` |
| `iterations` | integer | `24` | `1..=512` |
| `segment_size` | integer | `6` | `1..=64` |
| `temperature_start` | number | `1.0` | finite, `> 0` |
| `temperature_end` | number | `0.12` | finite, `> 0`, `<= temperature_start` |
| `reaction_factor` | number | `0.3` | finite, `(0,1]` |

## Full Recommended Payload

This is a practical high-quality payload for current research scenarios.

```json
{
  "units": "mm",
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 4.5,
    "trim_mm": {
      "left": 0.0,
      "right": 0.0,
      "top": 0.0,
      "bottom": 0.0
    },
    "time_limit_ms": 4000,
    "restarts": 5,
    "objective": "min_waste",
    "seed": 13,
    "layout_mode": "guillotine",
    "sla_profile": "balanced",
    "ga_profile": "balanced",
    "include_svg": true,
    "retry_strategy": "disabled",
    "profile_pool": {
      "enabled": true,
      "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8],
      "fill_penalty": 0.1,
      "max_lead_drop_pp": 0.8,
      "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21],
      "rescue_when_zones_gt": 5
    },
    "group_shift": {
      "enabled": true,
      "debug_artifacts": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  },
  "stock": [
    {
      "id": "mdf-2800x2070",
      "width_mm": 2070.0,
      "height_mm": 2800.0,
      "qty": 0
    }
  ],
  "items": [
    {
      "id": "panel-a",
      "width_mm": 700.0,
      "height_mm": 980.0,
      "qty": 2,
      "rotation": "allow_90",
      "pattern_direction": "none"
    },
    {
      "id": "core-a",
      "width_mm": 950.0,
      "height_mm": 1400.0,
      "qty": 2,
      "rotation": "allow_90",
      "pattern_direction": "none"
    }
  ]
}
```

## OptimizeResponse

Successful response:

```json
{
  "status": "ok",
  "summary": {
    "objective": "min_waste",
    "used_stock_count": 4,
    "total_waste_area_mm2": 123456.0,
    "waste_percent": 12.34,
    "time_ms": 842,
    "restarts_used": 5,
    "restarts_requested": 5,
    "used_seed": 13,
    "layout_mode": "guillotine"
  },
  "solutions": [],
  "unplaced_items": [],
  "artifacts": {
    "svg": "<svg .../>"
  }
}
```

### summary

| Field | Type | Description |
|---|---:|---|
| `objective` | string | Actual objective. |
| `used_stock_count` | integer | Number of sheets used. |
| `total_waste_area_mm2` | number | Total unused area. |
| `waste_percent` | number | Waste percentage. |
| `time_ms` | integer | Total service-side optimization time. |
| `restarts_used` | integer | Actual restarts executed. |
| `restarts_requested` | integer | Requested/default restarts before budget adjustment. |
| `used_seed` | integer | Seed used for result. |
| `layout_mode` | string | Actual layout mode. |
| `timeout_reason` | string/null | Present only on partial/timeout-related outcomes. |
| `restart_policy` | object/null | Present for standard restart telemetry. |
| `portfolio` | object/null | Present when portfolio is used. |
| `beam` | object/null | Present when beam is used. |
| `alns` | object/null | Present when ALNS is used. |
| `candidate_selection` | object/null | Top-K candidate scoring telemetry. |
| `profile_pool` | object/null | Present when profile pool is enabled. |
| `retry` | object/null | Present when smart retry performed recovery attempts. |
| `partition` | object/null | Present when partition mode is enabled. |
| `group_shift` | object/null | Present when group shift is enabled. |
| `vacuum` | object/null | Present when `layout_mode` is `"vacuum_table"`. Includes chosen direction, strategy, coverage, clearance, and first-sheet used bbox. |

### solutions[]

Each entry is one used sheet.

```json
{
  "stock_id": "mdf-2800x2070",
  "index": 0,
  "width_mm": 2070.0,
  "height_mm": 2800.0,
  "trim_mm": {
    "left": 0.0,
    "right": 0.0,
    "top": 0.0,
    "bottom": 0.0
  },
  "placements": []
}
```

| Field | Type | Description |
|---|---:|---|
| `stock_id` | string | Source stock id. |
| `index` | integer | Sheet index in output. |
| `width_mm` | number | Full sheet width. |
| `height_mm` | number | Full sheet height. |
| `trim_mm` | object | Trim used for this sheet. |
| `placements` | array | Parts placed on this sheet. |

### placements[]

```json
{
  "item_id": "panel-a",
  "instance": 1,
  "x_mm": 0.0,
  "y_mm": 0.0,
  "width_mm": 700.0,
  "height_mm": 980.0,
  "rotated": false,
  "pattern_direction": "none"
}
```

Coordinates use the top-left of the usable area after trim as origin.

| Field | Type | Description |
|---|---:|---|
| `item_id` | string | Input item id. |
| `instance` | integer | Instance number for this item. |
| `x_mm` | number | X coordinate in usable sheet area. |
| `y_mm` | number | Y coordinate in usable sheet area. |
| `width_mm` | number | Placed width. May differ from input width if rotated. |
| `height_mm` | number | Placed height. May differ from input height if rotated. |
| `rotated` | boolean | Whether part was rotated 90 degrees. |
| `pattern_direction` | string | Pattern direction copied to result. |

### unplaced_items[]

Present when some items could not be placed.

```json
{
  "item_id": "too-large",
  "instance": 1,
  "width_mm": 3000.0,
  "height_mm": 3000.0,
  "reason": "oversized"
}
```

Known reasons:

- `"oversized"`;
- `"qty_limit"`.

### artifacts

```json
{
  "svg": "<svg .../>",
  "group_shift_before_svg": "<svg .../>",
  "group_shift_diff_svg": "<svg .../>"
}
```

Fields:

- `svg`: final layout SVG. Present when `params.include_svg=true`.
- `group_shift_before_svg`: optional before-state SVG when `group_shift.debug_artifacts=true`.
- `group_shift_diff_svg`: optional diff SVG when `group_shift.debug_artifacts=true`.

## profile_pool telemetry

When `params.profile_pool.enabled=true`, response may include:

```json
{
  "preset": "balanced_quality",
  "profiles_requested": [0.2, 0.3, 0.4, 0.5],
  "rescue_zone_penalties_requested": [0.4],
  "candidates_total": 54,
  "candidates_completed": 54,
  "candidates_timed_out": 0,
  "candidates_failed": 0,
  "rescue_candidates_rejected_by_guard": 0,
  "seed_offsets_requested": [1, 2, 3, 5, 7, 8, 13, 21],
  "seed_offsets_used": [1, 2, 3, 5, 7, 8, 13, 21],
  "rescue_zone_penalties_used": [],
  "rescue_triggered": true,
  "rescue_when_zones_gt": 5,
  "rescue_when_max_corner_below_mm2": 300000.0,
  "rescue_accept_min_max_corner_mm2": 300000.0,
  "winner_seed": 26,
  "winner_zone_penalty": 0.5,
  "winner_visual_waste_regions": 9,
  "winner_waste_regions": 7,
  "winner_lead_util_pct": 92.5,
  "winner_max_corner_mm2": 450000.0,
  "winner_group_shift_opportunity_after_mm2": 0.0,
  "winner_group_shift_opportunity_delta_mm2": 197960.0,
  "winner_group_shift_contact_gain_mm": 1293.5,
  "max_lead_drop_pp": 0.8
}
```

Important fields:

- `winner_visual_waste_regions`: waste regions without part inflation; closer to visual review.
- `winner_waste_regions`: waste regions with kerf+spacing inflation.
- `winner_group_shift_contact_gain_mm`: contact-gain signal from group shift, useful for compactness evaluation.
- `rescue_triggered`: whether seed/profile rescue was used.

## group_shift telemetry

When `params.group_shift.enabled=true`, response may include:

```json
{
  "enabled": true,
  "time_ms": 0,
  "moves_applied": 4,
  "parts_moved": 6,
  "passes_run": 4,
  "corridor_closed_area_mm2": 248360.0,
  "contact_gain_mm": 1293.5,
  "corridor_opportunity_before_mm2": 248360.0,
  "corridor_opportunity_after_mm2": 0.0,
  "corridor_opportunity_delta_mm2": 197960.0,
  "max_shift_mm": 120.0
}
```

Important fields:

- `moves_applied`: accepted group shifts.
- `parts_moved`: total moved parts.
- `contact_gain_mm`: additional edge contact created toward anchor clusters.
- `corridor_closed_area_mm2`: closed/shifted corridor area.
- `corridor_opportunity_after_mm2`: remaining detected group-shift opportunity.

## vacuum telemetry

When `params.layout_mode = "vacuum_table"`, `summary.vacuum` is present:

```json
{
  "chosen_direction": "width",
  "strategy": "homogeneous",
  "placed_count": 11,
  "unplaced_count": 0,
  "coverage_ratio": 0.673469,
  "min_clearance_mm": 116.666667,
  "used_bbox": {
    "x_mm": 0.0,
    "y_mm": 0.0,
    "width_mm": 2800.0,
    "height_mm": 1050.0
  }
}
```

Important fields:

- `chosen_direction`: actual row direction selected after scoring.
- `strategy`: `homogeneous`, `general_shelf`, `mixed`, or `none`.
- `min_clearance_mm`: measured minimum part-to-part clearance in the final layout.
- `used_bbox`: first-sheet occupied bounding box. In a compact vacuum layout it should start near `x_mm=0`, `y_mm=0`; the main slack should remain outside the occupied cluster.

## ErrorResponse

All errors use:

```json
{
  "status": "error",
  "error_code": "VALIDATION_ERROR",
  "message": "time_limit_ms must be >= 100",
  "details": null
}
```

| Field | Type | Description |
|---|---:|---|
| `status` | string | Always `"error"`. |
| `error_code` | string | Machine-readable error code. |
| `message` | string | Human-readable message. |
| `details` | object/null | Optional structured details. |

HTTP statuses:

| Status | error_code | Meaning |
|---:|---|---|
| `400` | `VALIDATION_ERROR` | Invalid JSON/body parse error. |
| `413` | `CONSTRAINT_ERROR` | Body too large. |
| `422` | `VALIDATION_ERROR` | Request validation error. |
| `422` | `CONSTRAINT_ERROR` | Optimization/input constraint error. |
| `408` | `TIMEOUT` | Optimization timed out. |
| `429` | `OVERLOADED` | Too many concurrent optimize requests. |
| `500` | `INTERNAL` | Internal service error. |

## Validation Limits

Service/env defaults:

| Setting | Default | Description |
|---|---:|---|
| `PORT` | `8088` | HTTP port. |
| `MAX_BODY_BYTES` | `5242880` | Max request body size. |
| `MAX_INSTANCES` | `5000` | Max total item instances. |
| `DEFAULT_TIME_LIMIT_MS` | `2000` | Default optimization budget. |
| `DEFAULT_RESTARTS` | `10` | Default requested restarts. |
| `MAX_CONCURRENT_OPTIMIZE` | CPU count, min `1` | Concurrent optimize requests. |

Request validation:

- `stock` must not be empty.
- `items` must not be empty.
- `stock.len() <= 50`.
- `stock.id` values must be unique.
- `kerf_mm >= 0`.
- `spacing_mm >= 0`.
- `trim_mm.* >= 0`.
- Trim must leave positive usable area on every stock type.
- `time_limit_ms >= 100` if provided.
- `restarts >= 1` if provided.
- Total item instances must be `<= MAX_INSTANCES`.
- Stock dimensions must be `> 0`.
- Item dimensions must be `> 0`.
- Item `qty >= 1`.

## cURL Example

```bash
curl -sS -X POST "http://localhost:8088/v1/optimize" \
  -H "Content-Type: application/json" \
  --data-binary @examples/optimize_request.json
```

Minimal inline example:

```bash
curl -sS -X POST "http://localhost:8088/v1/optimize" \
  -H "Content-Type: application/json" \
  -d '{
    "units": "mm",
    "params": {
      "kerf_mm": 2.0,
      "spacing_mm": 1.0,
      "trim_mm": { "left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0 },
      "time_limit_ms": 1000,
      "restarts": 3,
      "objective": "min_waste",
      "layout_mode": "guillotine",
      "include_svg": false
    },
    "stock": [
      { "id": "sheet-1000", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 2 }
    ],
    "items": [
      { "id": "A", "width_mm": 200.0, "height_mm": 300.0, "qty": 2, "rotation": "allow_90", "pattern_direction": "none" },
      { "id": "B", "width_mm": 400.0, "height_mm": 400.0, "qty": 1, "rotation": "allow_90", "pattern_direction": "none" }
    ]
  }'
```
