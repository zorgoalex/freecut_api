# V74: Profile Pool Candidate Quality Audit

## Goal

Explain why V73 quality-aware profile_pool ordering did not change winners.
The key question: does the current profile_pool candidate set contain alternate
layouts with the same sheet/zone quality but better guarded group-shift
quality, or is the candidate set too narrow?

Branch: `feat/v74-profile-pool-candidate-quality-audit`.

Base: `origin/main` plus V70, V71, and V73. V72 code was not included because
it did not improve measured quality.

## Implementation

Added research harness:

`scripts/test_v74_profile_pool_candidate_quality_audit.py`

The script does not call `profile_pool`. Instead it runs each `zone_penalty`
profile as a standalone candidate:

- fixed seed;
- fixed `ga_override.zone_penalty`;
- fixed `ga_override.fill_penalty`;
- optional guarded `group_shift`;
- no retry strategy.

For each candidate it records:

- sheet count;
- visual and cut-gap waste-region counts;
- lead utilization;
- bottom-right corner remnant area;
- guarded `group_shift` quality score and deltas;
- contact gain and residual opportunity.

Then it compares:

- legacy profile_pool order:
  `sheets -> visual zones -> cut-gap zones -> residual -> contact -> delta -> lead -> corner`;
- V73 quality-aware order:
  `sheets -> visual zones -> cut-gap zones -> quality -> residual -> contact -> delta -> lead -> corner`.

## Validation

- `python -m py_compile scripts\test_v74_profile_pool_candidate_quality_audit.py`
  passed.
- `cargo fmt --check` passed.

## Benchmark

Command shape:

```bash
python scripts/test_v74_profile_pool_candidate_quality_audit.py \
  --port 8137 \
  --seeds 1 2 3 4 5 6 7 8 9 10 11 12 \
  --time-limit-ms 3000 \
  --restarts 3 \
  --out-dir ai_docs/tmp/v74_profile_pool_candidate_quality_audit
```

Artifacts:

- `ai_docs/tmp/v74_profile_pool_candidate_quality_audit/v74_candidate_metrics.csv`
- `ai_docs/tmp/v74_profile_pool_candidate_quality_audit/v74_candidate_metrics.json`
- `ai_docs/tmp/v74_profile_pool_candidate_quality_audit/v74_summary.md`

Results:

| candidate groups | rows | quality winner differed from legacy |
|---:|---:|---:|
| 24 | 144 | 0 |

Profile distribution of quality-rank winners:

| mode/profile | count |
|---|---:|
| off, 0.2 | 10 |
| off, 0.4 | 2 |
| on, 0.2 | 10 |
| on, 0.3 | 1 |
| on, 0.4 | 1 |

## Observations

- V73 did not change winners because the decisive criteria are still sheet
  count and region counts. The new quality tie-breaker is rarely reached.
- Most profiles produce identical or near-identical layouts for the decisive
  criteria on this fixture.
- The clearest pure-quality alternative was seed 10 `group_shift=on`:
  `zone_penalty=0.5` had higher quality score than the legacy winner, but it
  also had worse visual zones (11 instead of 10), so moving raw quality above
  zones would likely worsen visual topology.

## Conclusions

- The next bottleneck is candidate generation, not profile_pool ordering.
- A composite score that allows quality to override zones is risky until we
  have fixtures where the visual review confirms that the higher-quality
  candidate is actually better despite region-count loss.
- The next useful branch should generate structurally different candidates:
  targeted repair candidates, alternative constructive shelves/columns, or
  late-stage relocation around detected corridor gaps.
