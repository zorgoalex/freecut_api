# V72: Anchor Perimeter Group Shift

## Goal

Test whether `group_shift` should generate candidates around the perimeter of
the main dense anchor group, not only from cutline side groups. The intent was
to catch cases where one or more peripheral parts can be pulled toward the main
cluster and turn a visible internal corridor into edge waste.

Branch: `feat/v72-anchor-perimeter-group-shift`.

Base: `origin/main` plus V70 and V71.

## Implementation

V72 keeps the V71 quality guard mandatory:

- `topology_score_after >= topology_score_before`;
- `part_contact_mm_after > part_contact_mm_before`;
- combined `topology_score + 0.25 * part_contact_ratio` must improve.

Candidate generation was extended in two ways:

- anchor-perimeter band candidates around the largest connected anchor
  component;
- refined side-group candidates, where a normal cutline side group is narrowed
  to only the placements whose perpendicular span overlaps the anchor bbox.

New telemetry:

- `summary.group_shift.anchor_perimeter_candidates`;
- benchmark summary total `anchor_perimeter_candidates`.

## Validation

Tests:

- `cargo test group_shift -- --test-threads=1` passed: 11/11.
- `python -m py_compile scripts\test_v70_group_shift_remnant_audit.py`
  passed.

Benchmark setup:

- Script: `scripts/test_v70_group_shift_remnant_audit.py`.
- Fixture: `tests/fixtures/multisheet_varied_4sheets.json`.
- `layout_mode=guillotine`, `time_limit_ms=3000`, `restarts=3`.
- Seeds: 1..12.
- `group_shift.min_shift_mm=5`.
- Output artifacts:
  - `ai_docs/tmp/v72_anchor_perimeter_pass4/`
  - `ai_docs/tmp/v72_anchor_perimeter_pass8/`

## Results

Comparison against V71 on the same fixture/seeds:

| run | moved | improved | worsened | moves | parts | rejected | perimeter candidates | delta score | delta topology | delta contact | elapsed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| V71 pass4 | 11 | 11 | 0 | 25 | 35 | 9 | n/a | +0.0185736 | 0 | +8844mm | 36.07s |
| V71 pass8 | 11 | 11 | 0 | 25 | 35 | 9 | n/a | +0.0185736 | 0 | +8844mm | 35.14s |
| V72 pass4 | 11 | 11 | 0 | 25 | 35 | 9 | 650 | +0.0185736 | 0 | +8844mm | 40.74s |
| V72 pass8 | 11 | 11 | 0 | 25 | 35 | 9 | 723 | +0.0185736 | 0 | +8844mm | 35.45s |

Per-seed counters show that V72 did generate new candidates. The pass8 run had
perimeter candidates on all tested seeds, with the largest counts on seeds 5
and 1. However, none of those candidates produced a better accepted move than
the existing V71 cutline side-group candidates.

## Conclusions

- V72 is a useful negative result, not a production improvement.
- The anchor-perimeter idea is directionally reasonable, but this concrete
  candidate source is redundant on the current benchmark fixture.
- V71's guard was important: the expanded search did not create regressions,
  but it also did not create measurable quality gain.
- Do not merge V72 as-is into production defaults. Keep V71 as the stronger
  current candidate.

## Next Hypotheses

- V73: move the V71 quality score into `profile_pool` scoring so candidate
  layouts are selected based on their guarded post-process quality.
- Add a targeted visual-gap fixture where a peripheral part or chain is not
  selected by existing cutline side-group candidates; re-test anchor-perimeter
  only if such a fixture exposes a real miss.
- Benchmark V71 guard cost on larger ladder fixtures before broad production
  rollout.
