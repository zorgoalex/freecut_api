# V59/V61 productionization A: `cut_quality` profile

Branch: `feat/freecut-quality-profile` (from `main` @ `2c86de6`).
Date: 2026-06-18.

## Goal

Collapse the two low-level heuristic post-process knobs (`consolidate`, `lns`)
into one high-level request param so callers pick a quality tier with a single
clear word instead of hand-tuning two objects. No optimizer-quality change — pure
API ergonomics over the existing V59/V61 machinery.

## Setup

- New param `params.cut_quality: fast | balanced | max` (snake_case enum).
- Expansion in `resolve_consolidate_config` / `resolve_lns_config`
  (`src/optimizer.rs`):
  - `fast`     -> no consolidate, no lns (floor only).
  - `balanced` -> `consolidate{max_window:3,max_passes:8}` (FFD-only).
  - `max`      -> `consolidate` + `lns{max_iters:4000,max_window:4}`.
- Precedence: an explicit `consolidate`/`lns` object always overrides the
  profile (including `enabled:false` to force a stage off under any tier).
- Profile applies to `engine=heuristic` only; ignored for `engine=ga` so the GA
  path is never silently altered. Absent `cut_quality` => behaviour exactly as
  before (no implicit post-process).
- Distinct from `sla_profile`/`ga_profile` (those budget GA restarts; unchanged).

## Intermediate Observations

- The whole feature is resolution-layer only — both post-processors already
  existed and are unchanged, so the never-regress guarantees carry over verbatim.
- `resolve_*` are only reached inside the `engine=heuristic` branch, so the
  explicit engine gate is defensive (and unit-tested) rather than load-bearing.

## Measurements

Prod profile (dev container at `--cpus 1.5 -m 512m`, `MAX_CONCURRENT_OPTIMIZE=2`),
single N50 job (524 parts, sheet 2070x2800, `time_limit_ms=60000`,
`engine=heuristic`):

| cut_quality | wall   | sheets | waste% | expands to            |
|-------------|--------|-------:|-------:|-----------------------|
| fast        | 285ms  | 45     | 13.2   | floor only            |
| balanced    | 301ms  | 45     | 13.2   | consolidate (FFD)     |
| max         | 12931ms| 43     | 9.2    | consolidate + lns 4k  |

Note: on this *single* N50 instance `balanced` found no improving consolidation
window, so it matched `fast` (45 sheets). Consolidation's headline `-11` is an
*aggregate* over the 35-job grid (floor 1101 -> 1091), not a per-instance
guarantee; the never-regress contract only promises sheets <= floor. `max`'s LNS
did pierce the floor here (-2 sheets, -4pp waste) at the expected deep cost.

## Visual Findings

N/A — no layout-shape change beyond what V59/V61 already produce.

## Tests

- 6 resolution unit tests in `optimizer::tests`: each tier's consolidate/lns
  presence; explicit-object override both directions (fast+explicit enables,
  max+`enabled:false` disables); `engine=ga` ignores `cut_quality`.
- 1 integration test `cut_quality_profile_expands_and_never_regresses`: drives
  the full request path, asserts telemetry presence per tier, monotone sheet
  ladder (max <= balanced <= fast), preserved unplaced set, and ga-path ignore.
- Gates green: `cargo build --release`, `cargo fmt --check`, `cargo test
  --release` (77 pass / 6 ignored), vendor `cargo test --release` (60 pass).

## Final Conclusion Candidate

`cut_quality` is a safe ergonomic wrapper: callers send one word, the service
expands it into the proven V59/V61 flag sets with explicit-object override
preserved. No optimizer behaviour changes; `engine=ga` and the no-param default
are untouched. Cost/benefit per tier is the existing post-process trade-off
(`fast` ms-floor, `balanced` ~tens-ms FFD consolidation, `max` seconds-deep LNS).
Ready to merge independently of the async-postprocess work.
