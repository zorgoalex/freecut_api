# V72: Anchor Perimeter Group Shift

## Goal

Test whether `group_shift` should generate candidates around the perimeter of
the main dense anchor group, not only from cutline side groups.

Branch: `feat/v72-anchor-perimeter-group-shift`.

Base: `origin/main` plus V70 and V71.

## Implementation

V72 kept the V71 quality guard mandatory:

- `topology_score_after >= topology_score_before`;
- `part_contact_mm_after > part_contact_mm_before`;
- combined `topology_score + 0.25 * part_contact_ratio` must improve.

Candidate generation was extended with:

- anchor-perimeter band candidates around the largest connected anchor
  component;
- refined side-group candidates, where a normal cutline side group is narrowed
  to placements whose perpendicular span overlaps the anchor bbox.

## Validation

- `cargo test group_shift -- --test-threads=1` passed: 11/11.
- `python -m py_compile scripts\test_v70_group_shift_remnant_audit.py`
  passed.

Benchmark fixture: `tests/fixtures/multisheet_varied_4sheets.json`, seeds
1..12, `layout_mode=guillotine`, `time_limit_ms=3000`, `restarts=3`,
`group_shift.min_shift_mm=5`.

Artifacts:

- `ai_docs/tmp/v72_anchor_perimeter_pass4/`
- `ai_docs/tmp/v72_anchor_perimeter_pass8/`

## Results

| run | moved | improved | worsened | moves | parts | rejected | perimeter candidates | delta score | delta topology | delta contact |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| V71 pass4 | 11 | 11 | 0 | 25 | 35 | 9 | n/a | +0.0185736 | 0 | +8844mm |
| V72 pass4 | 11 | 11 | 0 | 25 | 35 | 9 | 650 | +0.0185736 | 0 | +8844mm |
| V72 pass8 | 11 | 11 | 0 | 25 | 35 | 9 | 723 | +0.0185736 | 0 | +8844mm |

## Conclusions

- V72 is a useful negative result.
- The new candidate source generated many additional candidates but did not
  improve any measured aggregate over V71.
- Do not merge V72 as-is into production defaults. V71 remains the stronger
  current production candidate.
