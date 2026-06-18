# V70: Group Shift Remnant Audit

## Goal

Re-evaluate `group_shift` as a targeted remnant-quality post-process, not only
as a side-group move. The research question is whether single-part, group, and
chain-like shifts can mathematically explain the visual improvement: a torn
internal remnant becomes more connected and more peripheral.

## Initial Questions

- Can the current implementation move one part as a valid one-item group?
- Can it apply sequential moves around the perimeter of a dense part cluster?
- Which current telemetry is too local (`closed_area`, `contact_gain`) and which
  missing metrics are needed to measure remnant connectivity?
- What before/after metrics best prove that internal gaps were pushed outward?

## Candidate Metrics

- Connected free-space components without kerf inflation.
- Largest boundary-connected free component area.
- Largest corner-connected free component area.
- Internal free-space area inside the placement bounding box.
- Boundary contact ratio of the largest free component.
- Part-cluster bbox density and contact gain.
- Secondary/remnant fragmentation area after excluding the largest free component.

## Interim Notes

Work started from `origin/main` at `b55a9c0`.

Initial implementation read:

- Current `group_shift` can move a single part: `evaluate_group_shift_candidate`
  rejects empty selections and full-layout selections, but allows `selected.len()
  == 1`.
- It can already work as a sequential chain: `apply_group_shift_postprocess`
  applies the best move, recomputes opportunities, and repeats up to
  `max_passes`.
- Current candidate sources are cutline side-groups plus disconnected components
  shifted toward the largest anchor component.
- Current acceptance/ordering is local: corridor closed area, contact gain,
  shift size, and selected area. It does not directly measure whether the free
  remnant became more connected or more boundary/corner-connected.
- V70 should therefore focus on paired before/after remnant metrics first, then
  decide whether to extend candidate generation or acceptance.

## 2026-06-18 paired remnant/contact benchmark

Added `scripts/test_v70_group_shift_remnant_audit.py`.

The script runs `/v1/optimize` with `group_shift.debug_artifacts=true`, saves
before/after/diff SVG artifacts under `ai_docs/tmp`, parses the before/after
SVGs, and computes paired metrics:

- `topology_score`: free-space topology only. It combines largest
  boundary-connected free-space ratio, internal free-space ratio,
  secondary-fragment ratio, bbox void ratio, and component count.
- `part_contact_mm`: total adjacency length between parts at allowed
  `kerf_mm + spacing_mm` clearance. This is the metric that captures the visual
  "edge parts shifted toward the main group" effect.
- `part_contact_ratio`: `part_contact_mm / total_part_perimeter_mm`.
- `remnant_score`: `topology_score + 0.25 * part_contact_ratio`.

This separation matters: the first smoke test proved that pure topology can stay
unchanged while the visual layout clearly improves by increasing part contact.
For seed 1, topology deltas were zero, but `part_contact_mm` increased by
1826.5mm.

Benchmark setup:

- Fixture: `tests/fixtures/multisheet_varied_4sheets.json`.
- `layout_mode=guillotine`, `time_limit_ms=3000`, `restarts=3`.
- Seeds: 1..12.
- `group_shift.min_shift_mm=5`.
- Sweep: `max_passes=1`, `4`, `8`.
- Output artifacts:
  - `ai_docs/tmp/v70_group_shift_pass1/`
  - `ai_docs/tmp/v70_group_shift_pass4/`
  - `ai_docs/tmp/v70_group_shift_pass8/`
  - visual PNG checks:
    `ai_docs/tmp/v70_group_shift_visual_compare.png`,
    `ai_docs/tmp/v70_seed6_visual_compare.png`,
    `ai_docs/tmp/v70_seed9_visual_compare.png`.

Results:

| max_passes | moved seeds | improved | worsened | moves | parts moved | delta remnant score | delta topology | delta contact mm | closed area mm2 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 11 | 11 | 0 | 11 | 15 | +0.0102077 | 0 | +4860.5 | 558400 |
| 4 | 11 | 9 | 2 | 28 | 38 | -0.0083389 | -0.0235249 | +7231.0 | 1695720 |
| 8 | 11 | 9 | 2 | 29 | 39 | -0.0074568 | -0.0235249 | +7651.0 | 1720920 |

Interpretation:

- The user hypothesis is confirmed in a measurable way: `group_shift` often
  improves visual compactness by increasing `part_contact_mm`, even when
  topology/zone counts stay unchanged.
- `max_passes=1` is the safest tested setting: every moved seed improved the
  combined score, and there were no topology regressions.
- Multi-pass chain shifting is real but unsafe without a paired acceptance
  guard. Passes 4/8 increased total contact and closed area, but introduced two
  regressions (seed 6 and seed 9) where `largest_boundary_ratio/topology_score`
  dropped.
- Visual check of seed 6 and seed 9 matches the new metrics: later passes move
  peripheral strips/groups, but they can reshape the peripheral leftover in a
  way that is worse for usable remnant topology even when local
  `group_shift.contact_gain_mm` is positive.
- Therefore `closed_area` and local selected-vs-anchor `contact_gain` are not
  sufficient acceptance metrics. The next implementation hypothesis should
  score each candidate/pass with the paired `remnant_score` or at least apply a
  hard guard:
  `topology_score_after >= topology_score_before - epsilon` and
  `part_contact_mm_after > part_contact_mm_before`.

Recommended next branch:

- V71: add a guarded `group_shift` mode that evaluates each candidate on cloned
  placements before applying it. It should accept single-part and group shifts
  only when the paired remnant/contact score improves, and it should stop the
  chain as soon as the best remaining move fails the guard.
