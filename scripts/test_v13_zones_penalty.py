"""V13 hypothesis test: nested/guillotine mix + zones-penalised peel selection.

V11 showed nested gives +0.63pp lead util but +2.2 waste zones. V10's
epsilon-based tie-breaker (0.3pp) was too narrow — nested candidates often
win by >0.3pp. V13 applies a per-zone penalty: effective_util = densest_util
- (zones-1) * 0.3pp. So a 3-zone nested candidate needs to be >0.6pp denser
than a 1-zone guillotine candidate to win.

Compares V10/V11 baseline vs V13 on 30 seeds.
"""
import json, os, sys, urllib.request, time
from collections import deque

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

OUT_DIR = os.environ.get("FREECUT_OUT_DIR",
    os.path.join(os.path.dirname(__file__), "..", "ai_docs", "tmp", "best_layouts_v13"))
os.makedirs(OUT_DIR, exist_ok=True)
PORT = os.environ.get("FREECUT_PORT", "8088")

with open(os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 10000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
base_req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
SHEET_BUDGET = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
base_req["params"]["partition"] = {"enabled": True}
if SHEET_BUDGET:
    base_req["params"]["partition"]["sheet_budget_ms"] = SHEET_BUDGET
OUT_DIR = os.environ.get("FREECUT_OUT_DIR", OUT_DIR)
os.makedirs(OUT_DIR, exist_ok=True)

KERF = 6.0

def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    r = urllib.request.Request(
        f"http://localhost:{PORT}/v1/optimize",
        data=payload, headers={"Content-Type": "application/json"})
    resp = urllib.request.urlopen(r, timeout=240)
    return json.loads(resp.read())

def sheet_geometry(sol):
    """Per-sheet: util%, n waste regions (>=5k mm2), largest region fill%, corner rect mm2."""
    trim = sol.get("trim_mm", {})
    W = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
    H = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
    pieces = sol.get("placements", [])
    used = sum(p["width_mm"] * p["height_mm"] for p in pieces)
    util = used / (W * H) * 100.0
    g = 10.0
    nx, ny = int(W // g), int(H // g)
    occ = [[False] * nx for _ in range(ny)]
    for p in pieces:
        x0, y0 = p["x_mm"], p["y_mm"]
        x1, y1 = x0 + p["width_mm"], y0 + p["height_mm"]
        for j in range(max(0, int(y0 // g)), min(ny, int((y1 + g - 1) // g))):
            for i in range(max(0, int(x0 // g)), min(nx, int((x1 + g - 1) // g))):
                cx, cy = (i + 0.5) * g, (j + 0.5) * g
                if x0 - KERF <= cx <= x1 + KERF and y0 - KERF <= cy <= y1 + KERF:
                    occ[j][i] = True
    seen = [[False] * nx for _ in range(ny)]
    regions = []
    for j in range(ny):
        for i in range(nx):
            if occ[j][i] or seen[j][i]:
                continue
            q = deque([(i, j)]); seen[j][i] = True; cells = []
            while q:
                ci, cj = q.popleft(); cells.append((ci, cj))
                for di, dj in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    a, b = ci + di, cj + dj
                    if 0 <= a < nx and 0 <= b < ny and not occ[b][a] and not seen[b][a]:
                        seen[b][a] = True; q.append((a, b))
            area = len(cells) * g * g
            if area >= 5000:
                xs = [c[0] for c in cells]; ys = [c[1] for c in cells]
                bbox = ((max(xs) - min(xs) + 1) * g) * ((max(ys) - min(ys) + 1) * g)
                regions.append((area, area / bbox * 100.0))
    regions.sort(reverse=True)
    lefts = {0.0} | {p["x_mm"] + p["width_mm"] for p in pieces if p["x_mm"] + p["width_mm"] < W}
    corner = 0.0
    for L in lefts:
        mb = 0.0
        for p in pieces:
            if p["x_mm"] < W and p["x_mm"] + p["width_mm"] > L:
                mb = max(mb, p["y_mm"] + p["height_mm"])
        corner = max(corner, (W - L) * (H - mb))
    return dict(util=util, n_regions=len(regions),
                top_fill=regions[0][1] if regions else 100.0, corner=corner)

results = []
t_start = time.time()
for seed in range(1, 31):
    try:
        data = call_optimize(seed)
    except Exception as e:
        print(f"  seed={seed:2d}: ERROR {e}")
        continue
    sols = data.get("solutions", [])
    n_sheets = len(sols)
    geo = [sheet_geometry(s) for s in sols]
    utils = [g["util"] for g in geo]
    min_util = min(utils) if utils else 0.0
    total_regions = sum(g["n_regions"] for g in geo)
    max_corner = max((g["corner"] for g in geo), default=0.0)
    sum_corner = sum(g["corner"] for g in geo)
    s_utils = sorted(utils, reverse=True)
    lead_util = sum(s_utils[:-1]) / max(1, len(s_utils) - 1) if len(s_utils) > 1 else min_util
    sel = data.get("summary", {}).get("candidate_selection", {})
    retry = data.get("summary", {}).get("retry", {}) or {}
    ptn = data.get("summary", {}).get("partition") or {}
    results.append({
        "seed": seed, "sheets": n_sheets,
        "min_util": round(min_util, 2),
        "lead_util": round(lead_util, 2),
        "utils": [round(u, 1) for u in utils],
        "n_waste_regions": total_regions,
        "max_corner_mm2": round(max_corner),
        "sum_corner_mm2": round(sum_corner),
        "winner_corner_free_mm2": sel.get("winner_corner_free_area_mm2", 0.0),
        "attempts": retry.get("attempts", 1),
        "partition_applied": ptn.get("applied", False),
        "densest_zones": ptn.get("densest_zones", []),
        "data": data,
    })
    mark = "OK " if n_sheets == 4 else "5s "
    dz = results[-1]["densest_zones"]
    print(f"  seed={seed:2d}: {mark}sheets={n_sheets}, lead={lead_util:5.2f}%, "
          f"regions={total_regions}, max_corner={max_corner/1e3:.0f}k, "
          f"peel={results[-1]['partition_applied']}, zones={dz}")

elapsed = time.time() - t_start
n = max(1, len(results))
n_4 = sum(1 for r in results if r["sheets"] == 4)
avg_lead = sum(r["lead_util"] for r in results) / n
avg_min = sum(r["min_util"] for r in results) / n
avg_regions = sum(r["n_waste_regions"] for r in results) / n
avg_max_corner = sum(r["max_corner_mm2"] for r in results) / n
n_regions_le_4 = sum(1 for r in results if r["n_waste_regions"] <= 4)
n_corner_300k = sum(1 for r in results if r["max_corner_mm2"] >= 300_000)

print(f"\nV13x Summary (30 seeds): elapsed {elapsed:.0f}s")
print(f"  4-sheet rate:                 {n_4}/{len(results)}")
print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%")
print(f"  Avg min util:                 {avg_min:.2f}%")
print(f"  Avg waste regions per layout: {avg_regions:.1f} (target: 4 = one per sheet)")
print(f"  Avg max corner rect:          {avg_max_corner/1e3:.0f}k mm2")
print(f"  Layouts with <=4 regions:     {n_regions_le_4}/{len(results)}")
print(f"  Layouts max_corner>=300k:     {n_corner_300k}/{len(results)}")
n_peel = sum(1 for r in results if r['partition_applied'])
print(f"  Peeling applied:              {n_peel}/{len(results)}")

ranked = sorted([r for r in results if r["sheets"] == 4],
                key=lambda r: (r["n_waste_regions"], -r["lead_util"]))
for i, r in enumerate(ranked[:5], start=1):
    svg = r.get("data", {}).get("artifacts", {}).get("svg", "") if isinstance(r.get("data"), dict) else ""
    stem = f"rank_{i:02d}_zones{r['n_waste_regions']}_seed_{r['seed']}"
    if svg:
        with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
            f.write(svg)
    d = {k: v for k, v in r.items() if k != "data"}
    with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
        json.dump(d, f, indent=2, ensure_ascii=False)
    print(f"  rank {i}: seed={r['seed']}, zones={r['n_waste_regions']}, "
          f"corner={r['max_corner_mm2']/1e3:.0f}k, lead={r['lead_util']}%, "
          f"utils={r['utils']}, densest_zones={r['densest_zones']}")

fmt = "v13_{:.1f}pp".format(float(os.environ.get("ZONES_PENALTY_PP", "0.8")))
summary_name = f"{fmt}_summary.json"
with open(os.path.join(OUT_DIR, summary_name), "w") as f:
    json.dump({"results": [{k: v for k, v in r.items() if k != "data"} for r in results]}, f, indent=2)
print(f"Saved to {OUT_DIR}")