# V73: Nested remnant-aware LNS acceptance — REJECTED

Branch: `feat/nested-remnant-accept` (from `feat/remnant-telemetry`).
Date: 2026-06-18.

## Goal

Phase 1 Component 2 from the nested brainstorming: extend the LNS equal-sheet
acceptance so nested layouts prefer a more consolidated offcut, using the V72
remnant metric to validate. Target: raise nested `mean_sheet_largest_free_frac`
toward guillotine without regressing sheet count.

## Change tried

In `lns_refine`, the equal-sheet branch accepts a repack that increases
`max_sheet_free_area` (concentrate free onto one sheet → sets up a sheet drop).
For nested, also accept a move that keeps `max_sheet_free_area` (`free2 >= cur_free`)
and increases `corner_free_area_units` (already computed in `build_candidate`),
to pull free space out of internal staircase notches.

## Measurement (rigorous A/B, seed sweep, window=6)

The first single-seed read looked positive, so it was checked across 4 seeds on
nested N35 `cut_quality=max` with `lns.max_window=6` (the V71 production setting):

| | mean_frac avg | sheets (seeds 42 / 7 / 99 / 123) |
|---|---:|---|
| baseline | 0.894 | 30 / 30 / 30 / 31 |
| Component 2 | 0.916 | 30 / **31** / **31** / 31 |

- Remnant gain is marginal (+0.022 avg) and inside the seed spread (0.82–0.94).
- **Sheet regression on 2 of 4 seeds**: Component 2 used 31 sheets where the
  baseline used 30. The corner-aware acceptance diverts the LNS off the
  trajectory that would have dropped a sheet. `free2 >= cur_free` does not prevent
  it — accepting a locally corner-better state changes which window is explored
  next, and that can cost a whole sheet.

Sheet count is the #1 lexicographic priority (never regress). A move that trades
a within-noise remnant gain for an occasional +1 sheet is disqualified.

## The real finding

At `max_window=6` (V71) the **baseline** nested remnant is already at parity with
guillotine: across the same 4 seeds, baseline `mean_sheet_largest_free_frac`
averages 0.894 vs guillotine ~0.90. The V72 gap (nested 0.795 vs guillotine
0.900) was measured at `max_window=4` — i.e. the visual fragmentation was the
*same small-window artifact* that V71 already fixed. Widening the LNS destroy
window to 6 closed both the sheet-count gap (V71) and the remnant gap.

So Component 2 chases an already-closed gap and risks a sheet doing it.

## Decision

Component 2 **rejected**; the `lns_refine` change is reverted (no code shipped on
this branch). The V72 remnant metric stays (it is what proved both the original
gap and that V71 closed it). No nested remnant work is needed beyond V71.

Possible safer follow-up if remnant is ever pushed further: a post-LNS pass doing
only same-sheet-count corner moves (cannot regress sheets by construction). Not
pursued now — headroom is within noise at window=6.
