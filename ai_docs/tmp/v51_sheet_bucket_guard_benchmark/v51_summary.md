# V51 sheet-count bucket guard benchmark

cut_gap_mm: 6.5

| case | visual0 | cut_gap | lead % | min % | zp | telemetry visual/cut |
|---|---:|---:|---:|---:|---:|---:|
| v51_seed_11 | 6 | 7 | 92.8327 | 89.2086 | 0.4 | 6/7 |
| v51_seed_13 | 7 | 6 | 90.8212 | 4.4218 | 0.8 | 7/6 |

Interpretation: this is a quick branch-local smoke benchmark.  It specifically checks whether V51 avoids the V50 regression where a 5-sheet high-lead candidate could beat the minimum-sheet bucket.
