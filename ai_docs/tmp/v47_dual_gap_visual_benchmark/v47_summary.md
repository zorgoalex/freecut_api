# V47 dual-gap visual benchmark

cut_gap_mm: 7.0
cases: 8
gap-sensitive cases: 2

| case | visual0 | cut_gap | delta | lead % | min % |
|---|---:|---:|---:|---:|---:|
| v41c_seed11_old | 5 | 5 | 0 | 94.6014 | 83.9024 |
| v41c_seed11_new | 4 | 4 | 0 | 94.2247 | 85.0325 |
| v41c_seed13_old | 5 | 5 | 0 | 94.6014 | 83.9024 |
| v41c_seed13_new | 4 | 4 | 0 | 93.3871 | 87.5452 |
| v43_seed13_hard_guard | 6 | 5 | -1 | 94.6014 | 83.9024 |
| v31_seed08_group_shift | 9 | 10 | 1 | 90.1878 | 6.9556 |
| v30_seed2_group_shift_off | 11 | 11 | 0 | 88.1137 | 15.2518 |
| v30_seed2_group_shift_on | 11 | 11 | 0 | 88.1137 | 15.2518 |

Conclusion: use both `zones_visual0` and `zones_cut_gap` in future paired audits. When they diverge, automatic scoring must not replace visual review.
