# V71: Nested-aware LNS window for large N

Branch: `feat/nested-aware-lns` (from `main` @ `b55a9c0`).
Date: 2026-06-18.

## Goal

Close the nested parity gap V70 found: at N50, `cut_quality=max` left nested at
44 sheets / 11.2% while guillotine reached 43 / 9.2%. Hypothesis 1 from V70: the
nested repack needs nested-specific tuning to hit the same floor at scale.

## Setup

- Service binary built from this branch; prod part mix, sheet 2070x2800,
  `engine=heuristic`, generous `time_limit_ms` so the LNS iteration count binds,
  not the deadline. Harness `/tmp/nested_parity.py` (hard metrics + placement
  visual proxy).
- The LNS params (`max_window`, `max_iters`, `window_ga_ms`) are exposed on the
  API, so the sweet spot was found by sweeping explicit params first, then baked
  into the `cut_quality=max` profile expansion.

## Intermediate Observations (nested N50 sweep)

| config | wall | sheets | waste% |
|---|---:|---:|---:|
| baseline `max_window=4 max_iters=4000` | 6.7s | 44 | 11.2 |
| `max_iters=8000` (win 4) | 12.5s | 44 | 11.2 |
| `max_iters=16000` (win 4) | 24.9s | 44 | 11.2 |
| `max_window=6 max_iters=4000` | **9.9s** | **43** | **9.2** |
| `max_window=6 max_iters=8000` | 14.8s | 43 | 9.2 |
| `max_window=5 max_iters=6000` | 7.8s | 44 | 11.2 |
| `max_window=8 max_iters=8000` | 23.3s | 44 | 11.2 |
| `max_window=4 window_ga_ms=30` | 6.5s | 44 | 11.2 |

Key findings:
- More iterations alone do NOT help (still 44 at 8k/16k). The binding constraint
  is the destroy-window size, not the iteration budget — nested already finishes
  4000 iters with time to spare.
- The destroy window is the lever: `max_window=6` reaches 43 / 9.2% (guillotine
  parity). The threshold is sharp — `max_window=5` still misses (44), and
  `max_window=8` overshoots back to 44 (FFD cannot repack 8 pooled sheets well).
- `window_ga_ms` did not help.
- Minimal winning config: `max_window=6, max_iters=4000` — 43 in ~9.9s.

Why nested specifically: the nested (free-rectangle) repack must pool a wider
window (6 sheets) to find the rearrangement that drops to 43; guillotine finds it
at 4. Guillotine at `max_window=6` also stays 43 but only gets slower (21.6s vs
13.8s), so guillotine keeps 4.

## Change

`resolve_lns_config` (`src/optimizer.rs`) — the `cut_quality=max` LNS window is
now mode-aware: `nested` => `max_window=6`, `guillotine` => `max_window=4`
(`max_iters=4000` unchanged). Only the profile expansion is affected; an explicit
`params.lns.max_window` still overrides for either mode. Models doc updated.

## Measurements (via profile, mode-aware window baked in)

`cut_quality=max`, guillotine vs nested:

| case | wall | sheets | waste% | frag_all | free_all |
|---|---:|---:|---:|---:|---:|
| nested N50 (was 44/11.2) | 9.8s | **43** | **9.2** | 1.7 | 0.91 |
| guillotine N50 | 13.9s | 43 | 9.2 | 1.4 | 0.94 |
| nested N35 | 10.9s | 30 | 9.2 | 1.6 | 0.92 |
| nested N20 | 10.5s | 17 | 9.2 | 1.8 | 0.90 |

- Hard-metric parity now reached at N50: nested 43 / 9.2% = guillotine.
- No regression at N20/N35 (LNS is monotone in sheet count).
- Nested deep is still faster than guillotine (9.8s vs 13.9s at N50).

## Tests

- Unit `cut_quality_max_lns_window_is_mode_aware`: nested -> window 6, guillotine
  -> window 4, explicit `lns.max_window` overrides both.
- Updated `cut_quality_max_resolves_to_consolidate_and_lns` to assert the
  guillotine default window (4).
- Gates: `cargo build --release`, `cargo fmt --check`, `cargo test --release`
  (79 pass / 6 ignored).

## Final Conclusion Candidate

The nested N50 parity gap was a neighbourhood-size problem, not an
iteration-budget problem: nested LNS needs `max_window=6` (vs guillotine 4) to
reach the same floor on large jobs, and that alone closes 44 -> 43 / 9.2%.
Implemented as a mode-aware `cut_quality=max` window with no guillotine change
and no regression at smaller N. Visual remnant is still not at parity (nested
frag 1.7 vs guillotine 1.4) — that remains hypothesis 2 (mode-aware
visual/remnant objective), out of scope here.
