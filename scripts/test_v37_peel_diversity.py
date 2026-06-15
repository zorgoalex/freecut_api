"""V37 hypothesis test: per-round peel zone penalty diversity.

V13 hardcoded zones_penalty_pp=0.8 for every peel round.  V37 varies it
per round: early rounds use low values (pure density), later rounds ramp
up (waste consolidation).  Default schedule: [0.0, 0.4, 0.8].

Compares:
  A) V22 baseline (peel_zone_penalties disabled = old 0.8 constant)
  B) V37 default schedule [0.0, 0.4, 0.8]
  C) V37 aggressive ramp [0.0, 0.6, 1.0]
  D) V37 slow ramp [0.0, 0.2, 0.4, 0.8]

Run: python scripts/test_v37_peel_diversity.py
Requires a running freecut server on FREECUT_PORT (default 8088).
"""
import json, os, sys, urllib.request, time
from collections import deque

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

PORT = os.environ.get("FREECUT_PORT", "8088")
SHEET_BUDGET = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
SEEDS = range(1, 31)
KERF = 6.0

FIXTURE_PATH = os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures", "multisheet_varied_4sheets.json")
with open(FIXTURE_PATH) as f:
    base_req = json.load(f)

# Configurations to compare
CONFIGS = {
    "A_baseline_08": None,  # no peel_zone_penalties → uses hardcoded default schedule
    "B_v37_default": [0.0, 0.4, 0.8],
    "C_v37_aggressive": [0.0, 0.6, 1.0],
    "D_v37_slow": [0.0, 0.2, 0.4, 0.8],
}

OUT_BASE = os.environ.get("FREECUT_OUT_DIR",
    os.path.join(os.path.dirname(__file__), "..", "ai_docs", "tmp", "best_layouts_v37"))
os.makedirs(OUT_BASE, exist_ok=True)


def build_request(config_name, peel_schedule, seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["time_limit_ms"] = 10000
    req["params"]["restarts"] = 5
    req["params"]["layout_mode"] = "guillotine"
    req["params"]["include_svg"] = True
    req["params"]["seed"] = seed
    req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
    req["params"]["retry_strategy"] = "smart"
    req["params"]["max_retry_attempts"] = 3
    partition = {"enabled": True}
    if SHEET_BUDGET:
        partition["sheet_budget_ms"] = SHEET_BUDGET
    if peel_schedule is not None:
        partition["peel_zone_penalties"] = peel_schedule
    req["params"]["partition"] = partition
    return req


def call_optimize(req):
    payload = json.dumps(req).encode()
    r = urllib.request.Request(
        f"http://localhost:{PORT}/v1/optimize",
        data=payload, headers={"Content-Type": "application/json"})
    resp = urllib.request.urlopen(r, timeout=240)
    return json.loads(resp.read())


def sheet_geometry(sol):
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
                regions.append(area)
    regions.sort(reverse=True)
    lefts = {0.0} | {p["x_mm"] + p["width_mm"] for p in pieces if p["x_mm"] + p["width_mm"] < W}
    corner = 0.0
    for L in lefts:
        mb = 0.0
        for p in pieces:
            if p["x_mm"] < W and p["x_mm"] + p["width_mm"] > L:
                mb = max(mb, p["y_mm"] + p["height_mm"])
        corner = max(corner, (W - L) * (H - mb))
    return dict(util=util, n_regions=len(regions), corner=corner)


def run_config(config_name, peel_schedule):
    print(f"\n{'='*60}")
    print(f"Config: {config_name}  schedule={peel_schedule or 'default [0.0,0.4,0.8]'}")
    print(f"{'='*60}")
    results = []
    t_start = time.time()
    for seed in SEEDS:
        req = build_request(config_name, peel_schedule, seed)
        try:
            data = call_optimize(req)
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
        s_utils = sorted(utils, reverse=True)
        lead_util = sum(s_utils[:-1]) / max(1, len(s_utils) - 1) if len(s_utils) > 1 else min_util
        ptn = data.get("summary", {}).get("partition") or {}
        results.append({
            "seed": seed, "sheets": n_sheets,
            "min_util": round(min_util, 2),
            "lead_util": round(lead_util, 2),
            "utils": [round(u, 1) for u in utils],
            "n_waste_regions": total_regions,
            "max_corner_mm2": round(max_corner),
            "partition_applied": ptn.get("applied", False),
            "densest_zones": ptn.get("densest_zones", []),
            "peel_zone_penalties_used": ptn.get("peel_zone_penalties_used", []),
            "data": data,
        })
        mark = "OK " if n_sheets == 4 else "5s "
        print(f"  seed={seed:2d}: {mark}sheets={n_sheets}, lead={lead_util:5.2f}%, "
              f"regions={total_regions}, corner={max_corner/1e3:.0f}k, "
              f"zones={ptn.get('densest_zones', [])}")

    elapsed = time.time() - t_start
    n = max(1, len(results))
    n_4 = sum(1 for r in results if r["sheets"] == 4)
    avg_lead = sum(r["lead_util"] for r in results) / n
    avg_min = sum(r["min_util"] for r in results) / n
    avg_regions = sum(r["n_waste_regions"] for r in results) / n
    avg_max_corner = sum(r["max_corner_mm2"] for r in results) / n
    n_regions_le_4 = sum(1 for r in results if r["n_waste_regions"] <= 4)
    n_peel = sum(1 for r in results if r["partition_applied"])

    print(f"\n{config_name} Summary ({n} seeds, {elapsed:.0f}s):")
    print(f"  4-sheet rate:                 {n_4}/{n}")
    print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%")
    print(f"  Avg min util:                 {avg_min:.2f}%")
    print(f"  Avg waste regions:            {avg_regions:.1f}")
    print(f"  <=4 regions:                  {n_regions_le_4}/{n}")
    print(f"  Avg max corner rect:          {avg_max_corner/1e3:.0f}k mm2")
    print(f"  Peeling applied:              {n_peel}/{n}")

    # Save top 5 ranked SVGs
    out_dir = os.path.join(OUT_BASE, config_name)
    os.makedirs(out_dir, exist_ok=True)
    ranked = sorted([r for r in results if r["sheets"] == 4],
                    key=lambda r: (r["n_waste_regions"], -r["lead_util"]))
    for i, r in enumerate(ranked[:5], start=1):
        svg = r.get("data", {}).get("artifacts", {}).get("svg", "") if isinstance(r.get("data"), dict) else ""
        stem = f"rank_{i:02d}_zones{r['n_waste_regions']}_seed_{r['seed']}"
        if svg:
            with open(os.path.join(out_dir, stem + ".svg"), "w", encoding="utf-8") as f:
                f.write(svg)
        d = {k: v for k, v in r.items() if k != "data"}
        with open(os.path.join(out_dir, stem + ".json"), "w", encoding="utf-8") as f:
            json.dump(d, f, indent=2, ensure_ascii=False)

    summary = {
        "config": config_name,
        "schedule": peel_schedule,
        "n_seeds": n,
        "elapsed_s": round(elapsed),
        "n_4_sheet": n_4,
        "avg_lead_util": round(avg_lead, 2),
        "avg_min_util": round(avg_min, 2),
        "avg_regions": round(avg_regions, 1),
        "n_regions_le_4": n_regions_le_4,
        "avg_max_corner_k": round(avg_max_corner / 1e3),
        "peel_applied": n_peel,
    }
    with open(os.path.join(out_dir, "summary.json"), "w") as f:
        json.dump({"summary": summary, "results": [{k: v for k, v in r.items() if k != "data"} for r in results]}, f, indent=2)

    return summary


if __name__ == "__main__":
    summaries = []
    for name, schedule in CONFIGS.items():
        s = run_config(name, schedule)
        summaries.append(s)

    print(f"\n{'='*60}")
    print("COMPARISON TABLE")
    print(f"{'='*60}")
    print(f"{'Config':<25} {'4-sheet':>8} {'lead%':>8} {'min%':>8} {'regions':>8} {'<=4reg':>8} {'corner_k':>10}")
    for s in summaries:
        print(f"{s['config']:<25} {s['n_4_sheet']:>5}/{s['n_seeds']:<3} "
              f"{s['avg_lead_util']:>7.2f}% {s['avg_min_util']:>7.2f}% "
              f"{s['avg_regions']:>7.1f} {s['n_regions_le_4']:>5}/{s['n_seeds']:<3} "
              f"{s['avg_max_corner_k']:>8.0f}k")
    print(f"\nAll results saved to {OUT_BASE}")
