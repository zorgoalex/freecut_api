#!/usr/bin/env python3
"""V60 lower-bound analysis for the Freecut cutting-stock floor.

Goal (decided with the user): measure how close the V61 anytime-LNS result is to
optimal, to decide whether a full Gilmore-Gomory column-generation solver is
worth building.

Two lower bounds, both computed here with no external solver:

1. AREA bound (rigorous): ceil(sum part_area / sheet_area). Valid because kerf
   and trim only ever increase the sheet count, never decrease it. This is also
   exactly the Gilmore-Gomory LP optimum when the pricing oracle is the
   *continuous* knapsack (area) relaxation -- i.e. the rigorous LP bound you get
   for free.

2. Gilmore-Gomory LP bound with GEOMETRY-AWARE pricing (indicative): real CG,
   solving the dual master LP with a hand-rolled simplex and pricing new
   single-sheet patterns with a rotation-aware shelf packer that maximises the
   dual-weighted value. Because the pricing packer is heuristic (exact 2D
   knapsack is NP-hard), this is an *estimate* of the geometric LP bound, not a
   certified bound -- it is >= area bound and a good indicator of the true gap.

The part mix and sheet match scripts/loadtest.py (furniture benchmark).
"""
import math
from itertools import count

SW, SH = 2070.0, 2800.0
SHEET = SW * SH
KERF = 3.2

# (id, w, h, base_qty, rotation) -- mirrors loadtest.py BASE_PARTS.
BASE_PARTS = [
    ("side", 600, 2000, 40, True), ("door", 396, 716, 90, True),
    ("shelf", 564, 300, 80, True), ("panel", 700, 980, 50, True),
    ("core", 950, 1400, 24, True), ("strip", 200, 2700, 18, True),
    ("base", 1200, 300, 40, True), ("box", 600, 600, 36, False),
    ("small", 350, 350, 70, False), ("tall", 300, 2000, 16, True),
    ("door2", 450, 900, 44, True), ("sq", 1000, 1000, 16, False),
    ("shelf2", 800, 500, 40, True), ("back", 500, 2070, 12, True),
    ("filler", 250, 250, 60, False),
]
FULL_NEED = sum(w * h * q for _, w, h, q, _ in BASE_PARTS) / SHEET


def order_types(N, frac=0.78):
    """Returns list of (w, h, can_rotate, demand) types scaled to N sheets."""
    scale = (frac * N) / FULL_NEED
    types = []
    for _, w, h, q, rot in BASE_PARTS:
        qty = max(1, round(q * scale))
        types.append((float(w), float(h), rot, qty))
    return types


def area_bound(types):
    area = sum(w * h * d for w, h, _, d in types)
    return math.ceil(area / SHEET), area / SHEET


# --- tiny dense simplex: maximize c.x s.t. A x <= b, x >= 0 (b >= 0) ----------
def simplex_max(c, A, b):
    """Standard-form max with sl... returns (opt_value, x). Bland's rule."""
    m, n = len(A), len(c)
    # tableau: [A | I | b], objective row at bottom.
    T = [row[:] + [1.0 if j == i else 0.0 for j in range(m)] + [b[i]]
         for i, row in enumerate(A)]
    obj = [-ci for ci in c] + [0.0] * m + [0.0]
    basis = [n + i for i in range(m)]
    for _ in range(10000):
        # entering: most negative obj coeff (Bland: first negative)
        piv_c = next((j for j in range(n + m) if obj[j] < -1e-9), None)
        if piv_c is None:
            break
        # ratio test
        piv_r, best = None, None
        for i in range(m):
            if T[i][piv_c] > 1e-9:
                ratio = T[i][-1] / T[i][piv_c]
                if best is None or ratio < best - 1e-12:
                    best, piv_r = ratio, i
        if piv_r is None:
            return float("inf"), None  # unbounded
        # pivot
        pv = T[piv_r][piv_c]
        T[piv_r] = [v / pv for v in T[piv_r]]
        for i in range(m):
            if i != piv_r and abs(T[i][piv_c]) > 1e-12:
                f = T[i][piv_c]
                T[i] = [a - f * b2 for a, b2 in zip(T[i], T[piv_r])]
        f = obj[piv_c]
        obj = [a - f * b2 for a, b2 in zip(obj, T[piv_r])]
        basis[piv_r] = piv_c
    x = [0.0] * n
    for i in range(m):
        if basis[i] < n:
            x[basis[i]] = T[i][-1]
    return obj[-1], x


# --- rotation-aware shelf packer: max dual-weighted value on ONE sheet --------
def price_pattern(types, pi):
    """Greedy shelf pack maximising sum(pi_t) over placed parts on one sheet.
    Returns (counts list, total_pi_value)."""
    # value density per type (best orientation that fits)
    items = []
    for t, (w, h, rot, dem) in enumerate(types):
        if pi[t] <= 1e-12:
            continue
        items.append((pi[t] / (w * h), t, w, h, rot, dem))
    items.sort(reverse=True)  # densest dual value first
    counts = [0] * len(types)
    # shelves along SH (length); each shelf has a height (max part h), width SW
    shelves = []  # list of [used_w, shelf_h]
    used_len = 0.0

    def try_place(w, h):
        nonlocal used_len
        # existing shelves
        for s in shelves:
            if s[1] >= h - 1e-9 and s[0] + w + KERF <= SW + 1e-9:
                s[0] += w + KERF
                return True
        # new shelf
        if used_len + h + KERF <= SH + 1e-9:
            shelves.append([w + KERF, h])
            used_len += h + KERF
            return True
        return False

    for _, t, w, h, rot, dem in items:
        for _ in range(dem):
            placed = try_place(w, h)
            if not placed and rot:
                placed = try_place(h, w)
            if placed:
                counts[t] += 1
            else:
                break
    val = sum(counts[t] * pi[t] for t in range(len(types)))
    return counts, val


def gg_lp_bound(types, max_cols=400):
    n = len(types)
    dem = [t[3] for t in types]
    # initial patterns: singleton (max copies of one type on a sheet)
    patterns = []
    for t, (w, h, rot, d) in enumerate(types):
        # max copies of this type alone via shelf packer
        pi = [0.0] * n
        pi[t] = 1.0
        cnt, _ = price_pattern(types, pi)
        if cnt[t] == 0:
            cnt[t] = 1  # at least one fits (assumed)
        patterns.append(cnt[:])
    for _ in range(max_cols):
        # dual LP: max sum(dem_t pi_t) s.t. for each pattern p: sum a_pt pi_t <=1
        A = [[float(p[t]) for t in range(n)] for p in patterns]
        b = [1.0] * len(patterns)
        val, pi = simplex_max([float(d) for d in dem], A, b)
        if pi is None:
            return None
        cnt, pval = price_pattern(types, pi)
        if pval <= 1.0 + 1e-6:
            return val  # converged (w.r.t. heuristic pricing)
        patterns.append(cnt)
    A = [[float(p[t]) for t in range(n)] for p in patterns]
    val, _ = simplex_max([float(d) for d in dem], A, [1.0] * len(patterns))
    return val


# V61 measured sheet counts (consolidate+lns, FFD repack, max_iters=6000),
# from the dev-container A/B sweep on this benchmark.
V61 = {
    (20, 0.70): 16, (20, 0.78): 17, (20, 0.86): 19,
    (30, 0.70): 23, (30, 0.78): 26, (30, 0.86): 29,
    (40, 0.70): 31, (40, 0.78): 35, (40, 0.86): 38,
    (50, 0.70): 39, (50, 0.78): 43, (50, 0.86): 47,
}

if __name__ == "__main__":
    print(f"{'N':>3} {'frac':>5} | {'areaLB':>6} {'ggLP*':>6} {'V61':>4} "
          f"{'gap(area)':>9} {'gap(gg)':>7}")
    print("-" * 56)
    tA = tG = tV = 0.0
    for (N, frac), v in V61.items():
        types = order_types(N, frac)
        alb, exact = area_bound(types)
        gg = gg_lp_bound(types)
        gg_ceil = math.ceil(gg - 1e-6) if gg else alb
        tA += alb; tG += gg_ceil; tV += v
        flag = " INVALID(>V61)" if gg_ceil > v else ""
        print(f"{N:>3} {frac:>5.2f} | {alb:>6} {gg_ceil:>6} {v:>4} "
              f"{v-alb:>9} {v-gg_ceil:>7}{flag}")
    print("-" * 56)
    print(f"TOTAL areaLB={int(tA)} ggLP*={int(tG)} V61={int(tV)} | "
          f"gap(area)={int(tV-tA)} gap(gg)={int(tV-tG)}")
    print("\n* ggLP = Gilmore-Gomory LP bound with heuristic geometry-aware "
          "pricing.\n  It exceeds V61 everywhere -> INVALID as a lower bound "
          "(a valid LP bound\n  must be <= any feasible integer solution). The "
          "heuristic shelf packer\n  under-packs patterns, so the dual prices "
          "stay too high and the value\n  overshoots. This demonstrates that a "
          "faithful GG bound needs an exact /\n  near-exact 2D-knapsack pricing "
          "oracle (the NP-hard core) -- with the\n  cheap continuous (area) "
          "pricing GG collapses to the area bound. So the\n  AREA bound is the "
          "only valid bound available cheaply: V61 is <=3 sheets/job\n  (~7%) "
          "above it, and much of that residual is geometrically unrecoverable.")
