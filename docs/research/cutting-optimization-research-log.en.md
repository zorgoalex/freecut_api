# Cutting Optimization Research Log

<!-- research-log-sync-index
goal
base-fixture
metrics
old-metric-issues
quality-frame
history-to-v28
v29-v33-group-shift
v34-geometry-penalties
v39-group-shift-visual
v40-corridor-audit
v41-zone-penalty-profile-pool
workspace-state
research-approach-conclusions
next-hypotheses-v42-v53
working-rules
v55-v58-performance
v59-v61-next
v63-packingsolver
v64-constructive-portfolio
v65-cpsat-verifier
v66-sat-feasibility
v67-ladder-benchmark
v59-v61-productionization-a-cut-quality
v59-v61-productionization-b-async-postprocess
v72-remnant-telemetry
v73-nested-remnant-accept
-->

Language: English.

This document is the English synchronized companion to
`cutting-optimization-research-log.ru.md`. Both files must keep the same
`research-log-sync-index` and must be updated together when final research
findings are promoted from drafts.

## Goal

Freecut is a Rust HTTP service for 2D rectangular cutting optimization. The
practical goal is not only to minimize mathematical waste, but to produce dense,
inspectable layouts that are useful in manufacturing:

- use the minimum number of stock sheets;
- maximize useful density on each used sheet;
- avoid internal corridors and fragmented empty zones;
- keep the remaining offcut as one large reusable region, preferably near an
  edge or corner;
- support both `nested` and `guillotine` modes, but compare them with the same
  visual/remnant metrics;
- test new hypotheses in separate branches from `main`;
- store generated SVG/PNG artifacts under `ai_docs/tmp`, not in `C:/tmp`.

The key visual-quality requirement is that parts should form one compact mass.
If a small group of edge parts can be shifted toward the main mass and the inner
gap is pushed outward, that can be more valuable than merely attaching parts to
the stock edge.

## Base Fixture And Theoretical Limit

Primary benchmark: `tests/fixtures/multisheet_varied_4sheets.json`.

Important dimensions:

- stock: 2070 x 2800 mm;
- trim: 10 mm on every side;
- usable area: 2050 x 2780 = 5.699M mm2;
- total item area: about 20.96M mm2;
- area lower bound: 4 sheets;
- useful target: 4 sheets with no unplaced items and visually compact remnants.

The old poor baseline was 5 sheets and about 26.46% waste. The meaningful
material breakthrough is reaching 4 sheets and about 8.07% waste.

## Main Metrics

Metrics must be interpreted in layers:

- hard validity: no overlaps, all constraints respected, no unplaced parts;
- material objective: sheet count first, then waste percent;
- per-sheet utilization: minimum and average utilization across used sheets;
- visual/remnant objective: fewer internal corridors, larger connected remnant,
  compact anchored part group;
- runtime objective: online latency for normal API usage vs high-quality/offline
  mode for expensive jobs.

Sheet count dominates waste. A layout using fewer sheets is usually better even
when visual compactness is not perfect. Within the same sheet count, visual
remnant quality becomes important.

## Old Metric Issues

Several earlier experiments showed that numeric metrics can disagree with visual
inspection. A layout can have acceptable waste but still contain internal gaps
that split the remnant into unusable fragments. The metric system must avoid
rewarding layouts that look mathematically acceptable but are practically poor.

The main lesson is that scoring should not only ask "how much empty area exists",
but also "where is it" and "is it reusable as one connected region".

## Quality Frame

The working quality frame is:

1. Minimize sheet count.
2. Minimize total waste for that sheet count.
3. Prefer dense connected groups of parts.
4. Push empty space outward to the sheet boundary.
5. Penalize internal corridors and fragmented offcuts.
6. Keep runtime acceptable for the selected quality mode.

This frame keeps the user-visible goal aligned with algorithmic selection.

## Compressed History To V28

The early research stages explored GA parameters, zone penalties, profile pools,
layout scoring, and visual audits. The important cumulative finding was that
pure GA tuning was not enough. Some layouts needed post-processing or alternate
construction logic because the optimizer could converge to layouts with
unwanted internal gaps.

The work before V28 created enough instrumentation to compare layouts, but the
visual-quality objective still needed stronger operational logic.

## V29-V33: Group Shift

Group shift was introduced to address the visual problem where a corridor
between a small edge group and the main body remains even though a simple group
movement would remove it.

The important principle is group movement, not only single-part movement. Moving
one part can merely move the corridor elsewhere. Moving the whole small group
toward the main cluster can push the corridor to the outer edge and create a
better reusable remnant.

Visual inspection indicated that group shift was under-valued by the original
metrics. It should be treated as a strong post-processing idea and combined with
future engines rather than discarded.

## V34: Geometry Penalties

V34/V34b tested geometry penalties inside the GA. The direction helped expose
where scoring needed to change, but it did not fully solve the visual remnant
problem by itself. It was too easy to tune one numeric penalty while missing the
actual visual goal.

The practical takeaway is that geometry penalties should support selection, but
not replace explicit compactness/remnant logic.

## V39: Group Shift And Visual Metric Recheck

V39 re-evaluated group shift with visual metrics. The result reinforced that
previous metrics were too weak: visually better layouts could be scored as
neutral or worse.

The lesson is that group-shift effects must be measured with before/after
metrics such as gap corridor reduction, connected empty region size, and compact
group movement, not just waste percent.

## V40: Corridor/Compactness Audit

V40 focused on identifying corridors and compactness defects. This line of work
is important because it makes the visual problem measurable. A good audit should
detect empty lanes between groups and determine whether they can be pushed out
with low-cost moves.

## V41: Expanded Zone Penalty Profile Pool

V41 expanded profile-pool scoring with zone penalties. It helped search more
variants, but the deeper conclusion remained the same: scoring needs to prefer
compact part groups and usable remnants, not only lower raw waste.

## Workspace State

The research workflow now uses separate branches/worktrees for hypotheses.
Large generated inputs, SVGs, PNGs, solver outputs, and temporary dependencies
belong under `ai_docs/tmp` and are not tracked by Git.

The versioned research log now lives in `docs/research/`, while intermediate
branch notes should live in `docs/research/drafts/`.

## Research Approach Conclusions

The original research approach was broadly correct: profile pools, visual
metrics, group shift, and alternative engines are all relevant. The strongest
correction is that metric design had to be upgraded. The system must account for
visual remnant quality and connected offcuts, otherwise it can reject layouts
that are visibly better.

## Next Hypotheses V42-V53

The next-stage hypotheses focused on visual remnant scoring and profile-pool
selection:

- V42: audit visual remnant metrics;
- V43: include remnant score in profile-pool selection;
- V44: accept group shift by remnant score;
- V45: repair one bad sheet / merge inter-sheet zones;
- V46: parity tests for visual metrics;
- V47: visual benchmark set;
- V48: profile-pool dual-zone telemetry;
- V49: visual-zone-first profile-pool selection;
- V50: quick V49 profile-pool benchmark;
- V51: sheet-count bucket lead guard;
- V52: seed-offset rescue benchmark;
- V53: contact-aware group-shift signal in profile-pool.

The shared theme is to make visual quality visible to the optimizer and not only
to manual inspection.

## Working Rules

- New implementation experiments should be done in separate branches from
  `main`.
- Best SVG/PNG artifacts should be stored under `ai_docs/tmp`.
- Final conclusions should be written to the canonical research log.
- Intermediate notes should be written to `docs/research/drafts/` and promoted
  only after the hypothesis is complete.
- Russian and English research-log versions must stay synchronized.

## Performance / Latency / Scale Research V55-V58

V55-V58 tested whether Freecut can provide useful results quickly instead of
only relying on longer GA runs.

V55/H1 added a best-partial path via a synchronous heuristic seed. This made the
system more robust when the full optimizer is interrupted or time-limited.

V56/H2 added multi-heuristic construction seeds. This improved the floor of
layout quality and helped avoid obviously poor starting points.

V57/H3 tested parallel full-budget restarts. The result was neutral and should
not be merged as a major direction without stronger evidence.

V58/H5 added `engine=heuristic`, an instant non-GA path. It is useful for low
latency and as a baseline, but not sufficient as the only quality engine.

The synthesis from V55-V58 is that Freecut needs a portfolio of strategies:
fast construction for latency, GA/heuristics for normal use, and higher-quality
methods for hard jobs.

## V59-V61 Next

The promising next hypotheses were:

- V59: per-sheet decomposition;
- V60: better bin/sheet assignment for minimizing sheet count;
- V61: anytime GA/LNS behavior.

These directions are relevant because many failures are not small placement
mistakes but allocation and repair problems across sheets.

## V63 PackingSolver

V63 tested `fontanf/packingsolver` as an external C++ sidecar/alternative
engine.

Key integration facts:

- built locally under `ai_docs/tmp/external/packingsolver`;
- Freecut dimensions were scaled to integer units;
- effective gap `kerf + spacing = 6.5mm` was passed as `--cut-thickness 65`
  at scale 10;
- trims were represented as hard trims in the PackingSolver bins CSV;
- tree-search-only mode worked without an LP solver;
- HiGHS-enabled builds also worked when `--linear-programming-solver highs` was
  passed.

Main fixture result:

| Engine | sheets | waste% | time |
|---|---:|---:|---:|
| Freecut defaults | 5 | 26.46 | about 3s wall |
| Freecut `engine=heuristic` | 5 | 26.46 | about 25ms wall |
| PackingSolver tree-search, 2s budget | **4** | **8.07** | about 36ms wall |

Extended runs:

| Case | Freecut heuristic | PackingSolver |
|---|---:|---:|
| repeat-factor 5 / 200 items | 20 sheets | 20 sheets |
| repeat-factor 10 / 400 items | 40 sheets | **39 sheets** |

Conclusion: PackingSolver is a serious high-quality engine candidate. It can
find sheet-count improvements that the current Freecut engine misses. It is not
a drop-in replacement for low-latency large requests, but it is suitable for
`engine=packingsolver`, `quality_mode=high`, or an auto-quality portfolio.

## V64 Constructive Portfolio

V64 implemented an independent Python MaxRects-style constructive portfolio with
several sort orders and placement rules (`bssf`, `baf`, `bl`, `contact`). It did
not call `cut-optimizer-2d`.

Measured results:

| Case | Mode | candidates | sheets | waste% | lower bound |
|---|---|---:|---:|---:|---:|
| main fixture | deterministic only | 24 | 5 | 26.46 | 4 |
| main fixture | +200 random/noisy restarts | 824 | **4** | **8.07** | 4 |
| repeat-factor 10 | deterministic only | 24 | 40 | 8.07 | 37 |
| repeat-factor 10 | +100 random/noisy restarts | 424 | 40 | 8.07 | 37 |

Visual inspection confirmed that the 4-sheet result was valid and materially
strong, but not yet ideal for connected remnants. Some internal empty zones
remained.

Conclusion: V64 is important because it shows that a cheap internal engine can
escape the 5-sheet local minimum on the main fixture. Deterministic construction
alone was not enough; order diversity and placement-rule portfolio created the
win. For larger repeated batches it did not match PackingSolver, so it should be
used as an internal seed/portfolio engine, not as a full replacement.

## V65 CP-SAT Verifier

V65 tested OR-Tools CP-SAT with fixed sheet count and `NoOverlap2D`.

Cold CP-SAT was not effective on the 40-piece main fixture:

| Case | Mode | status | sheets | time |
|---|---|---|---:|---:|
| tiny `optimize_valid.json` | no hint | OPTIMAL | 1 | about 19ms |
| main fixture | no hint, 30s | UNKNOWN | 4 | about 30s |
| main fixture | no hint, 10s | UNKNOWN | 5 | about 10s |
| main fixture | V64 SVG hint | OPTIMAL | **4** | about 0.85s |
| main fixture | V64 SVG hint + `--fix-hint` | OPTIMAL | **4** | about 18ms |

Conclusion: CP-SAT should not be prioritized as a standalone optimizer for
normal jobs. Its strong role is validation: with a complete hint it can verify a
layout very quickly and can be used for tests, overlap/gap validation, and
small-instance exact checks.

## V66 SAT Feasibility

V66 estimated the scale of naive SAT/MaxSAT/exact-cover encodings.

Main 40-piece fixture:

| grid step | candidate vars | pair upper bound |
|---:|---:|---:|
| 100mm | 90,232 | 990,330,388 |
| 50mm | 349,800 | 14,881,999,452 |
| 10mm | 8,521,248 | 8,830,576,090,976 |
| 0.5mm | 3,367,732,800 | 1,379,257,841,659,171,520 |

Repeat-factor 10:

| grid step | candidate vars | pair upper bound |
|---:|---:|---:|
| 100mm | 8,346,460 | 938,863,061,950 |
| 10mm | 788,215,440 | 8,372,979,935,293,040 |
| 0.5mm | 311,515,284,000 | 1,307,819,119,935,102,965,600 |

Conclusion: naive grid SAT/MaxSAT is not practical for this service. A possible
future SAT/exact-cover direction is only hybrid: generate a small set of
candidates from V64/PackingSolver/current layouts and use exact selection,
repair, or verification.

## V67 Ladder Benchmark

V67 tested practical scale from small cases to about 50 sheets using deterministic
generated cases from the main fixture.

Main ladder, sheet count:

| LB | pieces | Freecut heuristic | V64-style constructive | PackingSolver 1s |
|---:|---:|---:|---:|---:|
| 8 | 68 | 9 | 8 | 8 |
| 15 | 141 | 16 | 16 | 15 |
| 20 | 203 | 21 | 21 | 20 |
| 25 | 251 | 27 | 26 | 26 |
| 40 | 414 | 43 | 43 | 42 |
| 50 | 527 | 54 | 53 | 52 |

Higher-budget PackingSolver:

| LB | pieces | Constructive | PackingSolver 5s | PackingSolver 10s |
|---:|---:|---:|---:|---:|
| 25 | 251 | 26 | **25** | not run |
| 40 | 414 | 43 | 41 | not run |
| 50 | 527 | 53 | 52 | **51** |

Conclusions:

- Jobs up to about 7 sheets are not the main problem; all engines normally hit
  the lower bound.
- The first practical split appears at LB=8: Freecut heuristic uses 9 sheets,
  while constructive and PackingSolver hit 8.
- From LB=15 upward, PackingSolver is consistently best.
- At LB=50 the best measured result is PackingSolver 10s: 51 sheets versus lower
  bound 50, while Freecut heuristic needs 54 and constructive needs 53.
- The portfolio direction is confirmed, but 50-sheet production quality needs
  more than short PackingSolver runs: high-quality mode, constructive seeds,
  compact/group-shift scoring, and possibly longer/offline budgets.

## V59/V61 Productionization A: `cut_quality` Profile

- Branch: `feat/freecut-quality-profile`; draft:
  `docs/research/drafts/2026-06-18-cut-quality-profile.md`.
- The low-level `consolidate`/`lns` post-process knobs were collapsed into one
  request parameter for `engine=heuristic`: `cut_quality: fast | balanced | max`.
- Profile mapping:
  - `fast` = floor only;
  - `balanced` = consolidate (FFD);
  - `max` = consolidate + lns (`max_iters=4000`).
- Explicit `consolidate`/`lns` objects override the profile; `engine=ga`
  ignores `cut_quality`; absent parameter keeps previous behavior.
- This is a resolution-layer wrapper over existing V59/V61 behavior, not a new
  optimizer-quality change; the never-regress contract still applies.
- Prod profile (1.5cpu/512m), single N50 (524 parts): `fast` 285ms/45 sheets,
  `balanced` 301ms/45 sheets, `max` 12.9s/43 sheets (-2, waste 13.2% -> 9.2%).
  On this single instance `balanced` matched `fast`; the headline consolidation
  `-11` is a grid-aggregate result, not this one-instance result.

## V59/V61 Productionization B: Async-Safe + Bounded Post-Process

- Branch: `feat/freecut-async-postprocess`; draft:
  `docs/research/drafts/2026-06-18-async-postprocess.md`.
- The synchronous consolidate+lns post-process was moved off the async runtime
  thread into `tokio::task::spawn_blocking`, mirroring GA restarts. Previously,
  deep (`lns`) requests blocked a tokio worker for the whole deadline.
- Concurrency decision: each request already holds its `optimize_semaphore`
  permit for its full lifetime, so deep jobs remain bounded by
  `MAX_CONCURRENT_OPTIMIZE`; no separate deep cap is needed.
- Admission queue: over-cap requests now wait for a permit up to
  `OPTIMIZE_QUEUE_WAIT_MS` (new config, default 60s) instead of returning an
  immediate `429`; `0` restores immediate reject. With live cap=1, two
  concurrent deep N30 jobs both returned 200, the second waited about 6s, no
  429.
- Prod profile (1.5cpu/512m, `MAX_CONCURRENT_OPTIMIZE=2`), two concurrent deep
  N50 jobs while polling `/health/ready`:
  - inline before `spawn_blocking`: health p95 **3964ms**, max timeout, **2
    timeouts**; both async workers were occupied and deep jobs serialized
    (~26.5s wall);
  - `spawn_blocking`: health p95 **7.8ms**, max 124.9ms, **0 timeouts**
    (~12.5s);
  - results were identical (N50 = **43 sheets**, deterministic); this is an
    execution-context change only.
- True deep-mode cost on the prod profile: N50 `max_iters=4000` is about
  **13.1s** wall on 1.5cpu versus about 2-3s on the 3-cpu dev box; keep
  `MAX_CONCURRENT_OPTIMIZE` small in production.

## V72: Honest Visual-Remnant Metric

- Branch: `feat/remnant-telemetry`; draft:
  `docs/research/drafts/2026-06-18-remnant-telemetry.md`.
- Motivation: the V70 nested visual gap used a crude external raster proxy.
  Built a trustworthy in-service metric and confirmed/refuted the gap.
- The existing candidate metrics measure free *area*, not *connectivity*:
  `bbox_void` said nested was at parity (7.08M vs 6.99M, +1.3%) and `corner_free`
  even favoured nested — neither can tell one big offcut from many staircase
  notches of equal area.
- New `remnant_metrics` (`summary.remnant`): rasterize each used sheet on a 20mm
  grid, flood-fill empty cells into connected regions. Reports `free_fragments`,
  `largest_free_mm2`, `largest_free_frac`, `mean_sheet_largest_free_frac`.
  Computed once at response build, gated on `include_svg`; O(cells), no hot-loop
  cost. Unit-tested (L-shape => 1 fragment/frac 1.0; central bar => 2/0.5).
- Finding — the gap IS real by connectivity. N35 `cut_quality=max`: guillotine
  free_fragments 49 / mean_sheet_largest_free_frac **0.900**; nested 65 /
  **0.795**. Visual confirmation (emptiest sheet rendered to `ai_docs/tmp`):
  guillotine = clean aligned columns + one L-shaped remnant; nested packs *more*
  parts (22 vs 12) with a large bottom remnant but small internal staircase
  notches between mismatched parts. Eye and metric agree; `bbox_void` did not.
- Conclusion: the connectivity metric corrects the misleading `bbox_void` parity
  and gives a measurable target (`mean_sheet_largest_free_frac`). Nested's failure
  mode is internal staircase notches, not a fragmented main offcut. Re-justifies a
  remnant-aware nested step with a real objective. Metric is mode-agnostic infra,
  useful independent of any nested fix.

## V73: Nested Remnant-Aware LNS Acceptance — Rejected

- Branch: `feat/nested-remnant-accept`; draft:
  `docs/research/drafts/2026-06-18-nested-remnant-accept.md`.
- Tried (Phase 1 Component 2): in `lns_refine`, let nested equal-sheet repacks
  also be accepted when they increase the consolidated corner remnant
  (`corner_free_area_units`) without reducing `max_sheet_free_area`, to pull free
  space out of internal staircase notches.
- Rigorous A/B (4-seed sweep, nested N35 `cut_quality=max`, `lns.max_window=6`):
  remnant gain marginal and within noise (`mean_sheet_largest_free_frac` 0.916 vs
  baseline 0.894, seed spread 0.82–0.94), **but the change regressed sheet count
  on 2 of 4 seeds** (31 vs 30) — the corner-aware acceptance diverts the LNS off
  a sheet-drop trajectory. Sheet count is the #1 priority, so this is disqualified.
- The real finding: at `max_window=6` the **baseline** nested remnant is already
  at parity with guillotine (mean ~0.894 vs ~0.90). The V72 gap (0.795) was a
  `max_window=4` artifact — V71's wider window closed both the sheet-count gap and
  the remnant gap. Component 2 chases an already-closed gap and risks a sheet.
- Decision: rejected; the `lns_refine` change is reverted (no code shipped). The
  V72 remnant metric stays. A safer future option (only if remnant is pushed
  further) is a post-LNS same-sheet-count corner pass that cannot regress sheets;
  not pursued now (headroom within noise at window=6).
