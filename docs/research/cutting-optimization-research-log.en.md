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
