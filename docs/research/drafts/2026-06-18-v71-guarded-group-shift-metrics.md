# V71: Guarded Group Shift Metrics

## Goal

Continue V70 by moving the paired remnant/contact metric from an external audit
script into the actual `group_shift` post-process. The target is not to maximize
the number of shifts, but to accept only shifts that improve visible compactness
without damaging usable remnant topology.

## Implementation

Branch: `feat/v71-guarded-group-shift-metrics`.

Base: `origin/main` plus V70 audit commit.

Code changes:

- Added internal `GroupShiftQualityMetrics`.
- Added 10mm-grid free-space topology score:
  - largest boundary-connected free-space ratio;
  - internal free-space ratio;
  - secondary free-space ratio;
  - bbox void ratio;
  - component-count penalty.
- Added total part-contact score at `kerf_mm + spacing_mm` clearance.
- Combined score:
  `topology_score + 0.25 * part_contact_ratio`.
- Added candidate hard guard:
  - `topology_score_after >= topology_score_before`;
  - `part_contact_mm_after > part_contact_mm_before`;
  - combined score must improve.
- Kept existing contact-first ordering among candidates that pass the guard.
  A first attempt with quality-first ordering broke the older test that prefers
  a high-anchor-contact move over a larger low-contact corridor.
- Extended `summary.group_shift` telemetry:
  - `quality_guard_rejections`;
  - `quality_score_before/after/delta`;
  - `topology_score_before/after/delta`;
  - `part_contact_before/after/delta_mm`.

## Validation

Tests:

- `cargo test group_shift -- --test-threads=1` passed: 11/11.
- `cargo test optimize_accepts_group_shift_and_reports_telemetry -- --test-threads=1` passed.
- Added focused unit tests:
  - topology loss is rejected even if contact rises;
  - moves without total part-contact gain are rejected.

Benchmark setup:

- Script: `scripts/test_v70_group_shift_remnant_audit.py`, extended with V71
  telemetry fields.
- Fixture: `tests/fixtures/multisheet_varied_4sheets.json`.
- `layout_mode=guillotine`, `time_limit_ms=3000`, `restarts=3`.
- Seeds: 1..12.
- `group_shift.min_shift_mm=5`.
- Output artifacts:
  - `ai_docs/tmp/v71_group_shift_guard_pass1/`
  - `ai_docs/tmp/v71_group_shift_guard_pass4/`
  - `ai_docs/tmp/v71_group_shift_guard_pass8/`

## Results

Compared with V70 on the same fixture/seeds:

| run | moved | improved | worsened | moves | parts | rejected | delta score | delta topology | delta contact |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| V70 pass4 | 11 | 9 | 2 | 28 | 38 | n/a | -0.0083389 | -0.0235249 | +7231mm |
| V70 pass8 | 11 | 9 | 2 | 29 | 39 | n/a | -0.0074568 | -0.0235249 | +7651mm |
| V71 pass4 | 11 | 11 | 0 | 25 | 35 | 9 | +0.0185736 | 0 | +8844mm |
| V71 pass8 | 11 | 11 | 0 | 25 | 35 | 9 | +0.0185736 | 0 | +8844mm |

Per-seed guard behavior:

- Seed 6: V70 regression removed; V71 accepted 1 move and rejected 2 later
  candidates.
- Seed 9: V70 regression removed; V71 accepted 2 moves and rejected 3 later
  candidates.
- Seed 11: V71 rejected 4 later candidates and stopped at 3 accepted moves.

## Conclusions

- V70's diagnosis was correct: local `contact_gain_mm` and `closed_area` are
  insufficient for safe multi-pass group shifting.
- V71 confirms that a cheap paired quality guard can keep the visual benefit of
  group_shift while eliminating the observed topology regressions on the tested
  seed ladder.
- The guard does not merely reduce work. On pass8 it accepted fewer moves
  (29 -> 25) and still produced a higher total contact delta (+7651mm ->
  +8844mm), with zero topology loss.
- `max_passes=4` and `max_passes=8` produced the same final result after guard,
  so the current practical setting remains `max_passes=4` unless a wider
  benchmark proves value in higher limits.

## Next Hypotheses

- V72: broaden candidate generation around anchor-group perimeter, but keep the
  V71 guard mandatory.
- V73: make the quality score selectable in profile_pool/winner scoring, so the
  optimizer can prefer layouts that are better after guarded group_shift.
- V74: benchmark guard cost on larger ladder fixtures (15, 25, 50 sheet scale)
  because candidate scoring uses 10mm flood-fill.
