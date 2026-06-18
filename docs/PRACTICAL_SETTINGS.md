# Freecut Practical Settings Guide

This guide separates Freecut settings into two groups:

- practical settings that are useful in a real production API;
- research settings that are useful for hypothesis testing, parameter sweeps, benchmark scripts, and visual analysis, but should not be regular production defaults.

## Short Summary

For normal practical use, the client usually needs to control only:

- `kerf_mm`;
- `spacing_mm`;
- `trim_mm`;
- `objective`;
- `layout_mode`;
- `time_limit_ms`;
- `restarts`;
- `include_svg`;
- `seed`, when reproducibility is required;
- `group_shift`, when post-process compaction of peripheral part groups is required.

The following settings should be treated as advanced or research-only by default:

- `ga_override`;
- manual `zone_penalties`;
- `seed_offsets`;
- `portfolio`;
- `beam`;
- `alns`;
- `partition`.

## Kerf vs Spacing

`kerf_mm` and `spacing_mm` both contribute to the distance between parts, but they mean different things.

```text
effective_gap_mm = kerf_mm + spacing_mm
```

### kerf_mm

`kerf_mm` is the physical width of the cut. It is the material removed by the tool.

Examples:

- a 3.2 mm saw blade;
- a 6.0 mm router bit;
- a laser cut with an effective 0.2 mm width.

If two parts are adjacent and the cut goes between them, the tool needs physical space. That space does not belong to either part; it becomes removed material.

Practical rule:

```json
"kerf_mm": 3.2
```

Set this to the real saw-blade width, router-bit diameter, or effective laser/plasma cut width.

### spacing_mm

`spacing_mm` is additional process clearance beyond tool width.

It is not the physical tool width. It represents extra distance required by the manufacturing process.

Reasons to use spacing:

- prevent parts from touching;
- account for chips, vibration, backlash, clamps, or material instability;
- leave tabs or bridges;
- leave a safety margin for the operator;
- reduce risk of damaging the neighboring part edge.

Practical rule:

```json
"spacing_mm": 0.0
```

when no extra clearance is required.

```json
"spacing_mm": 1.0
```

for a small process margin.

```json
"spacing_mm": 3.0
```

when the material or machine requires a noticeable extra gap.

### Examples

Saw blade 3.2 mm, no extra margin:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.0
}
```

Router bit 6 mm with 1 mm process margin:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 1.0
}
```

Laser cut 0.2 mm, parts can be placed very close:

```json
{
  "kerf_mm": 0.2,
  "spacing_mm": 0.0
}
```

The final gap between neighboring parts is:

```text
kerf_mm + spacing_mm
```

Example:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 1.0
}
```

Effective gap: `4.2 mm`.

### Practical Values

For CNC routing:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 0.5
}
```

or:

```json
{
  "kerf_mm": 6.0,
  "spacing_mm": 1.0
}
```

For panel saw cutting:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.0
}
```

or:

```json
{
  "kerf_mm": 3.2,
  "spacing_mm": 0.5
}
```

For laser cutting:

```json
{
  "kerf_mm": 0.1,
  "spacing_mm": 0.0
}
```

or:

```json
{
  "kerf_mm": 0.2,
  "spacing_mm": 0.0
}
```

If unsure, avoid inflating both values at once. Overstating `kerf_mm + spacing_mm` reduces packing density.

## Recommended Production Default

This profile is suitable for a working API where speed, stability, and predictable behavior matter.

```json
{
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 1.0,
    "trim_mm": {
      "left": 0.0,
      "right": 0.0,
      "top": 0.0,
      "bottom": 0.0
    },
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 2000,
    "restarts": 10,
    "sla_profile": "balanced",
    "ga_profile": "balanced",
    "include_svg": true,
    "retry_strategy": "smart"
  }
}
```

Rationale:

- `layout_mode: "guillotine"` is a safer default for production workflows where cut feasibility matters.
- `objective: "min_waste"` usually matches the goal of minimizing leftover material.
- `time_limit_ms: 2000` and `restarts: 10` provide a reasonable quality/time balance.
- `retry_strategy: "smart"` is useful in a real API: the service can run a recovery attempt if the first result is poor or incomplete.
- `include_svg: true` is useful for auditing. High-throughput integrations may set it to `false`.

## Production Quality Profile

Use this when the layout is more difficult and a higher response time is acceptable.

```json
{
  "params": {
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 4000,
    "restarts": 8,
    "sla_profile": "balanced",
    "ga_profile": "quality",
    "include_svg": true,
    "retry_strategy": "smart"
  }
}
```

Use it for:

- expensive material;
- jobs with many parts;
- cases where layout quality matters more than response time;
- layouts that will be reviewed by an operator.

Do not use it as the only global default for all requests, because it increases response time.

## Practical Group Shift Settings

`group_shift` is a post-process that moves peripheral groups of parts toward the main dense group. This is the mechanism studied in V29-V33 and later re-evaluated using contact/anchor metrics.

Recommended practical profile:

```json
{
  "params": {
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

These values match the effective V29-V33 mode:

- `min_shift_mm: 5.0` filters out tiny movements that do not matter visually;
- `max_passes: 4` is usually enough to move several groups, not just one part;
- `debug_artifacts` is not enabled, so the response does not become unnecessarily large.

Minimal equivalent:

```json
{
  "params": {
    "group_shift": {}
  }
}
```

When the `group_shift` object is present, current defaults are:

- `enabled = true`;
- `min_shift_mm = 5.0`;
- `max_passes = 4`;
- `debug_artifacts = false`.

## Group Shift For Visual Audit

When you need to see exactly what moved, enable debug SVG artifacts:

```json
{
  "params": {
    "include_svg": true,
    "retry_strategy": "disabled",
    "group_shift": {
      "enabled": true,
      "debug_artifacts": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

The response will include:

- `artifacts.svg`: final layout after group shift;
- `artifacts.group_shift_before_svg`: layout before group shift;
- `artifacts.group_shift_diff_svg`: visual diff of the shifts.

For honest before/after comparison, prefer:

```json
"retry_strategy": "disabled"
```

and set a fixed:

```json
"seed": 12345
```

This compares the same initial layout before and after the post-process, instead of comparing two different stochastic attempts.

## Stronger Group Shift

If you need more aggressive compaction of peripheral groups:

```json
{
  "params": {
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 3.0,
      "max_passes": 6
    }
  }
}
```

Use this carefully:

- `min_shift_mm: 3.0` allows more small moves;
- `max_passes: 6` allows more accepted shifts;
- quality-guarded group shift should reject moves that worsen remnant topology,
  but higher pass counts still cost extra CPU and should be benchmarked on your
  own request set.

For practical quality checks, inspect:

- `summary.group_shift.quality_score_delta`;
- `summary.group_shift.topology_score_delta`;
- `summary.group_shift.part_contact_delta_mm`;
- `summary.group_shift.quality_guard_rejections`;
- `summary.group_shift.anchor_perimeter_candidates`, if present, only as a
  candidate-source diagnostic;
- before/final/diff SVG when `debug_artifacts=true`.

For production default, prefer:

```json
{
  "min_shift_mm": 5.0,
  "max_passes": 4
}
```

## Practical Profile With Group Shift

Good working payload for normal use:

```json
{
  "units": "mm",
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 1.0,
    "trim_mm": {
      "left": 0.0,
      "right": 0.0,
      "top": 0.0,
      "bottom": 0.0
    },
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "time_limit_ms": 3000,
    "restarts": 3,
    "sla_profile": "balanced",
    "ga_profile": "balanced",
    "include_svg": true,
    "retry_strategy": "smart",
    "group_shift": {
      "enabled": true,
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  },
  "stock": [],
  "items": []
}
```

`stock` and `items` must be filled with real sheet and part data.

## When To Enable Group Shift

Enable it when:

- the SVG shows narrow corridors between the main group and edge parts;
- a more compact cluster and a more coherent remnant are important;
- an operator reviews the SVG;
- compactness matters, not only formal waste percentage.

Do not enable it blindly when:

- the layout is already compact and has no visible internal corridors;
- every millisecond of response time matters;
- the downstream system is not prepared for post-processed coordinates;
- you need strict baseline stability for benchmarking.

## Profile Pool In Practice

`profile_pool` runs several `zone_penalty` variants and selects a winner. It can improve layout quality, but it increases compute time.

Moderate practical version for difficult jobs:

```json
{
  "params": {
    "profile_pool": {
      "enabled": true,
      "preset": "balanced_quality",
      "max_lead_drop_pp": 0.8
    }
  }
}
```

Use it for:

- difficult orders;
- cases where the basic result visually fragments the remnant;
- workflows where 4-10 second responses are acceptable;
- expensive materials.

Do not make it the default for every request:

- profile pool multiplies internal runs;
- `seed_offsets` can increase runtime sharply;
- without visual review, a mathematically acceptable result can still be visually questionable.

## Profile Pool + Group Shift

For maximum quality at a higher runtime cost:

```json
{
  "params": {
    "time_limit_ms": 4000,
    "restarts": 5,
    "objective": "min_waste",
    "layout_mode": "guillotine",
    "retry_strategy": "disabled",
    "include_svg": true,
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
      "min_shift_mm": 5.0,
      "max_passes": 4
    }
  }
}
```

This is not a production default. It is a quality/research mode.

Results from V52/V53:

- `seed_offsets` can find a 4-sheet layout where the basic pool produced 5 sheets;
- `group_shift` can improve visual compactness;
- without an additional acceptance guard, `group_shift` can sometimes worsen zone counts.

Use this mode for difficult layouts, but inspect SVG and telemetry.

## Research-Only Settings

The following settings should not be exposed to normal users as standard controls.

### ga_override

```json
{
  "ga_override": {
    "epochs": 180,
    "breed_factor": 0.55,
    "survival_factor": 0.7,
    "top_k_candidates": 12,
    "zone_penalty": 0.4,
    "fill_penalty": 0.1
  }
}
```

Use only for:

- benchmarks;
- parameter tuning;
- regression research;
- comparing hypotheses.

For working API usage, prefer `ga_profile`.

### Manual zone_penalties

```json
{
  "profile_pool": {
    "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8]
  }
}
```

This is a research-level setting. For production, prefer:

```json
{
  "profile_pool": {
    "preset": "balanced_quality"
  }
}
```

### seed_offsets

```json
{
  "profile_pool": {
    "seed_offsets": [1, 2, 3, 5, 7, 8, 13, 21]
  }
}
```

Useful for research and expensive quality mode, but not as a normal default. Each offset multiplies candidate runs.

### debug_artifacts

```json
{
  "group_shift": {
    "debug_artifacts": true
  }
}
```

Useful for visual audit and development. In production, keep it `false` because it increases response size.

### portfolio, beam, alns

These modes are useful for experiments and alternative orchestration strategies.

Normal API clients should start with:

```text
POST /v1/optimize
```

not:

```text
POST /v1/optimize/beam
POST /v1/optimize/alns
```

### partition

`partition` can be useful for dense-first layout experiments, but it should not be a default until there is a stable case-selection policy.

## What To Show In A UI

Do not expose every internal parameter in a normal user interface.

Show:

- `layout_mode`: guillotine / nested;
- `objective`: minimum waste / minimum sheets;
- quality level: fast / balanced / quality;
- checkbox: compact peripheral part groups;
- checkbox: include SVG;
- optional seed for reproducibility.

Do not show to normal users:

- `ga_override`;
- `zone_penalties`;
- `fill_penalty`;
- `seed_offsets`;
- `rescue_when_zones_gt`;
- `beam_width`;
- `alns.temperature_*`;
- `reaction_factor`;
- `partition.sheet_budget_ms`.

## Mapping UI Quality Levels To API

### Fast

```json
{
  "time_limit_ms": 1000,
  "restarts": 4,
  "sla_profile": "fast",
  "ga_profile": "fast",
  "retry_strategy": "smart"
}
```

### Balanced

```json
{
  "time_limit_ms": 2000,
  "restarts": 10,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "retry_strategy": "smart"
}
```

### Quality

```json
{
  "time_limit_ms": 4000,
  "restarts": 8,
  "sla_profile": "balanced",
  "ga_profile": "quality",
  "retry_strategy": "smart",
  "profile_pool": {
    "enabled": true,
    "preset": "balanced_quality",
    "max_lead_drop_pp": 0.8
  }
}
```

### Quality + Group Compaction

```json
{
  "time_limit_ms": 4000,
  "restarts": 8,
  "sla_profile": "balanced",
  "ga_profile": "quality",
  "retry_strategy": "smart",
  "profile_pool": {
    "enabled": true,
    "preset": "balanced_quality",
    "max_lead_drop_pp": 0.8
  },
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

## Recommended Defaults For Integration

Single practical default:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 2000,
  "restarts": 10,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "include_svg": true,
  "retry_strategy": "smart"
}
```

Single practical quality default with group shift:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 3000,
  "restarts": 3,
  "sla_profile": "balanced",
  "ga_profile": "balanced",
  "include_svg": true,
  "retry_strategy": "smart",
  "group_shift": {
    "enabled": true,
    "min_shift_mm": 5.0,
    "max_passes": 4
  }
}
```

Research preset for searching high-quality layouts:

```json
{
  "objective": "min_waste",
  "layout_mode": "guillotine",
  "time_limit_ms": 4000,
  "restarts": 5,
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
}
```

## Operational Rules

1. For production, start with simple settings and inspect SVG.
2. Enable `group_shift` when visual gaps between edge groups and the main group matter.
3. Enable `debug_artifacts` only for analysis.
4. Use a fixed `seed` when comparing layouts.
5. Use `retry_strategy: "disabled"` for exact before/after benchmarks.
6. Use `retry_strategy: "smart"` for real user-facing APIs.
7. Do not expose raw GA and profile-pool internals to ordinary users.
8. Keep `profile_pool.seed_offsets` for expensive quality mode or research.
9. Treat `waste_percent` as necessary but not sufficient; inspect SVG for important jobs.
10. For group-shift quality, inspect `summary.group_shift.quality_score_delta`, `topology_score_delta`, `part_contact_delta_mm`, `quality_guard_rejections`, and before/diff SVG.
