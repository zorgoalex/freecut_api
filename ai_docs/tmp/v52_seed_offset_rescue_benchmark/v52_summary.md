# V52 seed-offset rescue benchmark

cut_gap_mm: 6.5

| case | sheets | visual0 | cut_gap | lead % | min % | zp | rescue | completed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| v52_seed_11 | 4 | 6 | 7 | 92.8327 | 89.2086 | 0.4 | True | 54 |
| v52_seed_13 | 4 | 9 | 7 | 92.5192 | 90.1491 | 0.5 | True | 54 |

Interpretation: this is a quick branch-local smoke benchmark.  It specifically checks whether seed-offset rescue can generate a better minimum-sheet candidate when ranking changes alone are not enough.
