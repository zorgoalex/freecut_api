# V76: Backfill "freeze-and-nibble" prototype — REJECTED

Branch: `feat/backfill-prototype` (from `feat/backfill-spike` / V75).
Date: 2026-06-18.

## Goal

V75 showed a real structural gap of +3/+4 sheets over the area lower bound at
large N that LNS plateaus at. Prototype a backfill / "freeze-and-nibble" pass to
capture it: keep the dense sheets, take the emptiest sheet, and relocate all of
its parts into the existing free space of the other sheets (no repacking of those
sheets, no rotation). If a whole sheet empties, sheet count drops by one.

## What was built (then reverted)

- `params.backfill { enabled, max_donors }`, run after `lns` in the
  `engine=heuristic` path.
- `free_rectangles(w, h, obstacles, gap)` — maximal empty rectangles per sheet,
  obstacles inflated by `gap` on all sides so any placement keeps clearance.
- `try_drain_sheet` — best-fit insert every donor-sheet piece into other sheets'
  free rects; succeeds only if all fit.
- `backfill_drain` — loop draining the emptiest sheets; accept only a strict
  sheet-count decrease (cannot regress or drop a part).
- 4 unit tests (free-rect correctness; drains a drainable layout 2→1; never
  regresses when no drain is possible) — all green.

## Measurement (LB40 / LB50, `cut_quality=max` ± backfill)

| target | mode | max | max+backfill |
|---|---|---:|---:|
| LB~40 | guillotine | 43 | **43** |
| LB~40 | nested | 43 | **43** |
| LB~50 | guillotine | 55 | **55** |
| LB~50 | nested | 56 | **56** |

Backfill captured **zero** additional sheet drops (wall +~50ms, so it ran and
found nothing). unplaced stayed 0, waste unchanged.

## Why — structural

After `lns`, the receiving sheets are packed near the floor (~95%+ full); their
free space is small staircase notches with no absorbing capacity. Draining the
emptiest sheet (~22 parts, fill ~0.54) requires all of its parts to fit into
those notches — they do not. Naive insert-without-repack is therefore weaker than
or equal to `lns`/`consolidate`: where there is room, the window repacks already
exploit it; where there is none, backfill cannot create it.

The unit test confirms the mechanism works when capacity exists (2 half-full
sheets → 1); it just never exists on real floor layouts.

## Decision

Rejected; code reverted (no shipped feature). The V75 structural gap is real but
needs **global repacking**, not local insertion — consistent with V67: an
external/global engine (PackingSolver reaches 51 at LB50 vs our 55) or a
constructive portfolio (V64), not a post-process. Backfill-with-repack would just
re-derive `lns`. Next real sheet-count lever is engine-level (PackingSolver
integration / V64 constructive portfolio), not backfill.
