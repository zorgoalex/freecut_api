# V70: Nested parity of V28+ approaches

Branch: `feat/nested-approach-parity` (from `main` @ `b55a9c0`).
Date: 2026-06-18.

## Goal

Decide how equally the approaches accumulated since V28 apply to improving the
**nested** (non-guillotine) layout logic, and empirically check the
nested-applicable ones in the current service. Criterion (per user): weigh hard
metrics (sheets, waste%) AND visual remnant quality equally. Scope: in-service
only — no external-engine rebuilds (V63 PackingSolver / V64 MaxRects /
V65 CP-SAT are classified by logic, not re-run here).

## Setup

- Both modes are first-class in code: every construction/post-process branch
  switches `LayoutMode::Nested => build_nested_heuristic()` /
  `Guillotine => build_guillotine_heuristic()`. Default mode is `guillotine`
  (`optimizer.rs:235,2214`). So nested is reachable for every in-service approach.
- Harness `/tmp/nested_parity.py`: runs each approach on guillotine and nested,
  N in {20,35,50}, prod part mix (sheet 2070x2800). Hard metrics from the API;
  visual proxy computed from returned placements.
- Visual proxy per sheet (20mm raster of usable area): `frag` = number of
  connected empty regions (4-neighbour flood fill); `free` = largest empty
  region / total empty (1.0 = one connected remnant). Reported as mean over used
  sheets (`_all`) and mean excluding the emptiest slack sheet (`_dns`). Lower
  `frag` + higher `free` = more reusable remnant.

## Phase 1 — classification of V28+ approaches by nested-applicability

| Approach (version) | Logic class | Applies to nested? | Equally by logic? |
|---|---|---|---|
| Group shift (V29–V33, V39, V44, V53) | geometric post-process on placed rects | yes | logic yes; effect differs (corridors are a guillotine artifact; nested fragments voids instead) |
| Geometry penalties (V34) | GA fitness scoring | yes | needs nested-aware calibration |
| Corridor/compactness audit (V40) | waste-shape metric | yes | semantics tuned on guillotine corridors |
| Zone-penalty profile pool (V41, V49) | candidate scoring/selection | yes | mostly equal; penalties calibrated on guillotine |
| Visual remnant metrics (V42, V46, V47) | measurement | yes | **not mode-aware** — never recalibrated for nested |
| Profile-pool selection / dual-zone (V43, V48, V50) | orchestration over candidates | yes | equal (mode-agnostic) |
| Sheet-count bucket guard (V51), seed-offset rescue (V52) | orchestration | yes | equal |
| Best-partial seed (V55), multi-heuristic seed (V56) | construction seeding | yes (uses nested heuristic) | equal |
| Parallel restarts (V57), engine=heuristic (V58) | runtime / fast path | yes | equal |
| Per-sheet decomp / bin assignment (V59 cons., V60) | geometric repack post-process | yes (nested repack) | logic equal; effect weaker at scale (see Phase 2) |
| Anytime LNS (V61) | geometric repack post-process | yes (nested repack) | logic equal; **worse local optimum at N50** |
| cut_quality wrapper (prod A) | resolution layer | yes | equal |
| async/queue (prod B) | runtime infra | yes | fully mode-agnostic |
| partition peeling (V8a) | dense-first construction | yes | equal (no sheet change on tested mix) |
| PackingSolver `rectangleguillotine` (V63) | external engine | **guillotine-specific as integrated** | would need PackingSolver `rectangle` for nested |
| MaxRects constructive portfolio (V64) | external constructive | yes — MaxRects is itself a nested/free-rect method | actually *more* nested-aligned |
| CP-SAT NoOverlap2D verifier (V65) | exact verifier | yes — NoOverlap2D has no guillotine constraint | nested-friendly |
| SAT feasibility scale (V66) | encoding estimate | yes | mode-agnostic |

Summary of Phase 1: nearly every V28+ approach is **logically applicable** to
nested — the engine already runs both modes for construction, post-process,
scoring and orchestration. Only the V63 binding (`rectangleguillotine`) is
guillotine-specific as wired; everything else either applies equally
(infra/orchestration/selection) or applies with effect/calibration differences
(the geometric repacks and the visual/remnant metrics). The visual-remnant
scoring line (V40–V53) was developed and tuned on guillotine corridor waste and
was **never made mode-aware** — that is the main gap for nested.

## Phase 2 — empirical parity (in-service, current `main` binary)

Post-process line (`engine=heuristic`, cut_quality tiers), guillotine vs nested:

| N (parts) | approach | mode | sheets | waste% | frag_all | free_all |
|---|---|---|---:|---:|---:|---:|
| 20 (208) | floor | guillotine | 18 | 14.3 | 1.5 | 0.99 |
| 20 | floor | nested | 18 | 14.3 | 1.9 | 0.88 |
| 20 | lns(max) | guillotine | 17 | 9.2 | 1.8 | 0.86 |
| 20 | lns(max) | nested | 17 | 9.2 | 2.2 | 0.84 |
| 35 (366) | floor | guillotine | 31 | 12.1 | 1.5 | 0.94 |
| 35 | floor | nested | 31 | 12.1 | 1.6 | 0.90 |
| 35 | lns(max) | guillotine | 30 | 9.2 | 1.6 | 0.90 |
| 35 | lns(max) | nested | 30 | 9.2 | 2.2 | 0.80 |
| 50 (524) | floor | guillotine | 45 | 13.2 | 1.3 | 0.99 |
| 50 | floor | nested | 45 | 13.2 | 1.6 | 0.93 |
| 50 | lns(max) | guillotine | **43** | **9.2** | 1.4 | 0.94 |
| 50 | lns(max) | nested | **44** | **11.2** | 1.6 | 0.93 |

(`consolidate`/`balanced` matched `floor` on every instance both modes — no
improving window on this mix; omitted for brevity.)

GA-line (`engine=ga`, N20):

| approach | mode | sheets | waste% | frag_all | free_all |
|---|---|---:|---:|---:|---:|
| ga floor | guillotine | 18 | 14.3 | 1.5 | 0.99 |
| ga floor | nested | 18 | 14.3 | 1.9 | 0.88 |
| ga + group_shift | guillotine | 18 | 14.3 | 1.6 | 0.98 |
| ga + group_shift | nested | 18 | 14.3 | 2.0 | 0.87 |
| ga + partition | guillotine | 18 | 14.3 | 1.5 | 0.99 |
| ga + partition | nested | 18 | 14.3 | 1.9 | 0.88 |

Observations:
- **Hard-metric parity holds at N20/N35**: nested gets the same floor and the
  same lns improvement (-1 sheet, same waste) as guillotine.
- **Parity breaks at N50**: nested lns reaches 44 sheets / 11.2% vs guillotine
  43 / 9.2%, i.e. one sheet worse. Nested lns also finished faster (6.2s vs
  13.6s) — it exhausted improving moves into a worse local optimum, so at scale
  the nested repack is weaker, not equivalent.
- **Visual remnant is never at parity**: nested starts more fragmented at the
  floor (frag 1.6–1.9 vs 1.3–1.5; free 0.88–0.93 vs 0.94–0.99) and the
  sheet-count-driven lns does not repair it (N35 nested free drops to 0.80).
  None of the in-service approaches has a nested-aware remnant objective.
- group_shift / partition produced no sheet change on either mode for this mix
  and only a negligible visual delta — they neither help nor hurt nested here.
- Nested construction is ~2–3x faster than guillotine at the same N (floor 71ms
  vs 223ms at N50; lns 6.2s vs 13.6s), so latency is not the nested blocker.

## Final Conclusion Candidate

By construction the V28+ machinery is **almost entirely applicable to nested** —
the code runs both modes for every in-service approach, so "which versions fit
nested" is "nearly all of them except the guillotine-specific external binding".
But applicable ≠ equally effective:

1. **Orchestration / runtime / selection approaches** (profile pool, seed
   rescue, fast path, cut_quality, async/queue, partition) apply to nested
   **equally** — they are mode-agnostic.
2. **Geometric repacks** (consolidate, lns, bin assignment) apply and give the
   **same hard-metric win at small/mid N, but degrade at scale** (nested lns is
   one sheet worse at N50). To reach guillotine-level sheet counts on large
   nested jobs the repack/LNS needs nested-specific tuning (more iters / better
   neighbourhood / stronger nested repack), not just the shared code path.
3. **Visual / remnant scoring** (V40–V53, group_shift) is the **least
   transferable**: it was calibrated on guillotine corridor waste, while nested
   produces fragmented voids. This metric line must be made mode-aware before it
   can improve nested remnant quality — currently nested is consistently more
   fragmented and no approach closes that gap.

So: most approaches transfer to nested in principle and on hard metrics at
moderate scale; the concrete nested-improvement work is (a) a stronger
nested-aware repack/LNS for large N, and (b) a mode-aware visual/remnant
objective. These two are the next hypotheses worth a dedicated branch.
