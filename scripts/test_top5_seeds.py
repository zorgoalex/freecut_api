"""Detailed test of 5 specific seeds to compare before/after tie-breaker."""
import json, requests, time, os

URL = "http://localhost:8088/v1/optimize"
FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
SHEET_AREA = (2070 - 20) * (2800 - 20)

with open(FIXTURE) as f:
    base_req = json.load(f)

# Original top-5 seeds
SEEDS = [1, 4, 5, 100, 106]

print("Testing with min-sheet-util tie-breaker (same seeds as top-5):")
print(f"{'='*80}")

for seed in SEEDS:
    req = json.loads(json.dumps(base_req))
    req["params"]["time_limit_ms"] = 10000
    req["params"]["restarts"] = 5
    req["params"]["layout_mode"] = "guillotine"
    req["params"]["seed"] = seed
    req["params"]["include_svg"] = False
    req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}

    r = requests.post(URL, json=req, timeout=30)
    data = r.json()
    sols = data.get("solutions", [])
    summary = data.get("summary", {})
    telemetry = summary.get("candidate_selection", {})
    portfolio = summary.get("portfolio", {})

    per_sheet = []
    for sol in sols:
        trim = sol.get("trim_mm", {})
        uw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        uh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        sheet_a = uw * uh
        used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
        pcs = len(sol.get("placements", []))
        util = used / sheet_a * 100 if sheet_a > 0 else 0.0
        per_sheet.append((util, pcs))

    utils = [u for u, _ in per_sheet]
    min_util = min(utils) if utils else 0
    max_util = max(utils) if utils else 0
    ss = " ".join(f"{u:.1f}({p})" for u, p in per_sheet)

    print(f"\nseed={seed}:")
    print(f"  sheets={len(sols)}, waste={summary.get('total_waste_area_mm2', 0):.0f}, "
          f"time={summary.get('time_ms', 0)/1000:.1f}s")
    print(f"  per-sheet util (pcs): {ss}")
    print(f"  min={min_util:.1f}%, spread={max_util-min_util:.1f}%")
    print(f"  portfolio: winner={portfolio.get('winner_strategy')}, "
          f"completed={portfolio.get('candidates_completed')}, "
          f"seed={portfolio.get('winner_seed')}")
    print(f"  telemetry: min_util_pct={telemetry.get('winner_min_sheet_util_pct')}, "
          f"candidates={telemetry.get('candidates_total')}, "
          f"rejected_min_util={telemetry.get('candidates_rejected_tie_min_util')}")
