# V73: Profile Pool Group Shift Quality

## Goal

Use the V71 guarded `group_shift` quality score inside `profile_pool` selection.
The hypothesis was that profile_pool should prefer candidates that produce a
better dense group and usable remnant after guarded group shift, not only
candidates with lower residual corridor opportunity or higher local contact.

Branch: `feat/v73-profile-pool-group-shift-quality`.

Base: `origin/main` plus V70 and V71. V72 code was intentionally not included
because V72 did not improve measured quality.

## Implementation

Added per-candidate fields:

- `group_shift_quality_score_after`;
- `group_shift_quality_score_delta`;
- `group_shift_topology_score_delta`;
- `group_shift_part_contact_delta_mm`.

Selection order became:

`used_stock_count -> visual_waste_regions -> waste_regions -> group_shift quality -> residual opportunity -> contact gain -> opportunity delta -> lead util -> max corner`

Added telemetry in `summary.profile_pool`:

- `winner_group_shift_quality_score_after`;
- `winner_group_shift_quality_score_delta`;
- `winner_group_shift_topology_score_delta`;
- `winner_group_shift_part_contact_delta_mm`;
- `quality_scoring_changed_winner`;
- `legacy_winner_seed`;
- `legacy_winner_zone_penalty`;
- `legacy_winner_group_shift_quality_score_after`;
- `legacy_winner_group_shift_quality_score_delta`.

The legacy-winner fields make each benchmark self-auditing: if
`quality_scoring_changed_winner=false`, V73 did not actually change the
selected profile_pool winner for that request.

## Validation

- `cargo test profile_pool -- --test-threads=1` passed: 18/18.
- `cargo test profile_pool_prefers_group_shift_quality_before_residual -- --test-threads=1`
  passed.
- `python -m py_compile scripts\test_v53_contact_guard_benchmark.py` passed.

## Benchmarks

Harness:

- `scripts/test_v53_contact_guard_benchmark.py`
- extended with `--out-dir` and V73 quality/legacy columns.

Artifacts:

- `ai_docs/tmp/v73_profile_pool_quality_benchmark/`
- `ai_docs/tmp/v73_profile_pool_quality_12seed_norescue/`

Seed 11/13 with seed-offset rescue:

| rows | quality changed winner | result |
|---:|---:|---|
| 4 | 0 | V73 matched V71 exactly: same sheets/zones/zp/contact. |

Seeds 1..12 without seed-offset rescue and without SVG:

| rows | quality changed winner | result |
|---:|---:|---|
| 24 | 0 | Current and legacy winner matched on every row. |

## Conclusions

- V73 is useful telemetry and a valid unit-level selection rule, but it is not
  a production quality improvement on the current benchmark fixtures.
- The current profile_pool candidates usually differ before the new quality
  tie-breaker is reached. Sheet count, visual zones, and cut-gap zones already
  select the same winner as the legacy order.
- This confirms the next bottleneck is candidate diversity or a higher-level
  composite quality gate, not just reordering existing tie-breakers.
- Keep V71 as the stronger production candidate. Treat V73 as diagnostic
  scaffolding for future profile_pool experiments.

## Next Hypotheses

- Generate candidate layouts that are intentionally equal on sheets/zones but
  differ in post-shift remnant quality.
- Test a composite quality gate that can compare visual-zone count and V71
  quality score together, instead of using quality only after zones tie.
- Add a targeted fixture where current profile_pool returns multiple plausible
  winners with visibly different group compactness.
