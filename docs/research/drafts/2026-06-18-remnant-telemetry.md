# V72: Honest visual-remnant metric (connected free-region analysis)

Branch: `feat/remnant-telemetry` (from `main` @ `b55a9c0`).
Date: 2026-06-18.

## Goal

V70 flagged a nested visual-remnant gap using a crude external raster proxy.
Before chasing a fix, build a trustworthy in-service metric so "remnant quality"
is measurable, and confirm (or refute) the gap against the optimizer's existing
metrics and against the eye.

## Why the existing metrics were not enough

`Candidate` already computes `bbox_void` (free area inside the part group's
bounding box) and `corner_free` (largest corner rectangle). On N35
`cut_quality=max` these said nested was **at parity** with guillotine:

| metric | guillotine | nested |
|---|---:|---:|
| bbox_void mm2 (density) | 6.99M | 7.08M (+1.3%) |
| corner_free mm2 | 6.87M | 6.98M (higher) |

Both measure free *area*, not *connectivity*. They cannot distinguish one big
reusable offcut from many small staircase notches of the same total area — which
is exactly the nested failure mode.

## The metric

`remnant_metrics` (`src/optimizer.rs`): rasterize each used sheet's usable area
on a 20mm grid, mark cells covered by any placement, flood-fill the empty cells
into connected regions (4-neighbour). Surfaced as `summary.remnant`:

- `free_fragments` — total connected empty regions across used sheets (lower is
  better);
- `largest_free_mm2` — single largest connected offcut;
- `largest_free_frac` — that as a fraction of all free area;
- `mean_sheet_largest_free_frac` — mean over used sheets of each sheet's
  largest-free fraction (per-sheet remnant quality).

Computed once at response build, gated on `include_svg` (same inspection intent;
skipped on the latency-sensitive `include_svg=false` path). O(cells); no hot-loop
cost. Unit-tested on known layouts (L-shaped free space => 1 fragment, frac 1.0;
central bar => 2 fragments, frac 0.5).

## Finding — the gap is real (connectivity, not area)

N35 `cut_quality=max`, the new metric:

| | sheets | free_fragments | mean_sheet_largest_free_frac |
|---|---:|---:|---:|
| guillotine | 30 | 49 | **0.900** |
| nested | 30 | 65 | **0.795** |

Unlike `bbox_void`, the connectivity metric shows nested IS meaningfully more
fragmented: more free regions and a lower per-sheet largest-free fraction.

Visual confirmation (rendered the emptiest sheet of each, `ai_docs/tmp/`):
- guillotine emptiest sheet: parts in clean aligned columns, free space is one
  clean L-shaped remnant, no internal notches;
- nested emptiest sheet: packs *more* parts (22 vs 12, fill 0.54 vs 0.59) with a
  large bottom remnant, but leaves several small internal staircase notches
  between heterogeneous parts.

So nested's big offcut is fine; the gap is small trapped staircase voids inside
the part group. The eye and the new metric agree; `bbox_void` did not see it.

## Conclusion

The honest connectivity metric was worth building: it corrects the misleading
`bbox_void` parity reading and gives a trustworthy target —
`mean_sheet_largest_free_frac` (nested 0.795 vs guillotine 0.900). The concrete
nested failure mode is internal staircase notches between mismatched parts, not a
fragmented main remnant. This re-justifies a remnant-aware nested step (Phase 1
Component 2: make the LNS equal-sheet acceptance prefer fewer fragments / higher
largest-free for nested), now with a measurable objective. Metric is
mode-agnostic infra and useful independent of any nested fix.
