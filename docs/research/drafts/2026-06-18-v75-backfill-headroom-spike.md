# V75: Backfill headroom spike (measurement-only)

Branch: `feat/backfill-spike` (from `main` @ `c54a9c5`).
Date: 2026-06-18.

## Goal

Before building a backfill / sheet-count post-optimization (the Phase 2 idea:
after compaction frees space on a sheet, pull parts from tail sheets to fill it
and possibly drop a sheet), measure whether the current LNS leaves real
sheet-count headroom at large N. If LNS is already at the area lower bound,
backfill has nothing to gain.

## Setup

- No code change. Current `main` binary (`cut_quality=max` machinery: heuristic
  floor + consolidate + LNS), exercised via explicit `lns` params to sweep the
  destroy window and iteration budget.
- Part mix scaled (loadtest `order(N, 0.78)`) so the area lower bound lands ~40
  and ~50 sheets. Area lower bound = ceil(total part area / sheet area)
  (loadtest uses trim=0, so the full sheet is usable).
- Harness: `/tmp/backfill_spike.py`. Budget 40s/run, both modes, windows {6,8},
  iters {4000,8000}.

## Results

| target | N | parts | area LB | best sheets (config) | gap |
|---|---:|---:|---:|---|---:|
| LB~40 | 50 | 524 | 40 | 43 (guillotine w6/w8; nested w6) | **+3** |
| LB~50 | 64 | 670 | 50 | 54 (guillotine w8) | **+4** |

Per-config (sheets):

| | LB~40 guil | LB~40 nested | LB~50 guil | LB~50 nested |
|---|---:|---:|---:|---:|
| w6 / 4000 | 43 | 43 | 55 | 56 |
| w8 / 8000 | 43 | 44 | 54 | 56 |

## Findings

- **Real headroom exists**: the current best sits +3 (LB40) to +4 (LB50) sheets
  above the area lower bound. This matches the V67 ladder finding (Freecut
  heuristic 54 vs LB 50 at the 50-sheet case).
- **Not a budget problem**: widening the window to 8 and doubling iters to 8000
  barely moves it — LB40 guillotine stays 43, LB50 only 55→54 (−1). The LNS is
  stuck in a structural local optimum, not starved of iterations.
- **Provably beatable**: V67 measured PackingSolver 10s reaching 51 at LB50 (vs
  our 54), so ~−3 sheets is achievable there — headroom the current internal
  engine cannot reach by tuning.
- nested is at parity with guillotine at LB40 (both 43, w6) but ~2 sheets worse
  at LB50 (56 vs 54); `w8` overshoots for nested (LB40 43→44), consistent with
  V71 (nested wants w6, not wider).

## Conclusion

Backfill (or any sheet-count post-optimization) targets a **real, structural**
gap of ~3–4 sheets over the area lower bound at large N, which more LNS
budget/window does not close and which an external solver demonstrably beats.
This justifies prototyping a backfill / freeze-and-nibble step (keep dense
sheets, aggressively empty tail sheets by pulling their parts into freed space
on denser sheets). Next: a backfill prototype on its own branch, measured against
this baseline (43 @ LB40, 54 @ LB50) and the area lower bound. Scope note: this
is a sheet-count lever, not a remnant/visual one (V71 already settled remnant).
