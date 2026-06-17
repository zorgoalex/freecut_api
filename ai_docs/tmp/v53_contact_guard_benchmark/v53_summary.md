# V53 contact-aware profile_pool benchmark

cut_gap_mm: 6.5

| case | sheets | visual0 | cut_gap | lead % | zp | winner contact | moves | actual contact | time ms | completed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| v53_seed_11_off | 4 | 6 | 7 | 92.8327 | 0.4 | 0.0 | None | None | 88129 | 54 |
| v53_seed_11_on | 4 | 7 | 9 | 92.8327 | 0.4 | 816.5 | 3 | 816.5 | 84148 | 54 |
| v53_seed_13_off | 4 | 9 | 7 | 92.5192 | 0.5 | 0.0 | None | None | 79486 | 54 |
| v53_seed_13_on | 4 | 9 | 7 | 92.5192 | 0.5 | 1293.5 | 4 | 1293.5 | 74217 | 54 |

Interpretation: compare *_off vs *_on for the same seed. The *_on rows include group_shift and expose winner/actual contact gain, so V53 can be judged against visual zones, cut-gap zones, and local anchor compaction.
