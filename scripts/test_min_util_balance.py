"""Test min-sheet-utilization tie-breaker effect.
Runs N seeds with portfolio mode and reports per-sheet utilization.
"""
import json, requests, time, statistics, os, sys

URL = "http://localhost:8088/v1/optimize"
FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
SEEDS = 20
SHEET_AREA = (2070 - 20) * (2800 - 20)  # usable after trim

with open(FIXTURE) as f:
    base_req = json.load(f)


def run_one(seed: int) -> dict:
    req = json.loads(json.dumps(base_req))
    req["params"]["time_limit_ms"] = 10000
    req["params"]["restarts"] = 5
    req["params"]["layout_mode"] = "guillotine"
    req["params"]["seed"] = seed
    req["params"]["include_svg"] = False
    req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
    t0 = time.time()
    r = requests.post(URL, json=req, timeout=30)
    elapsed = time.time() - t0
    data = r.json()
    sols = data.get("solutions", [])
    per_sheet = []
    for sol in sols:
        trim = sol.get("trim_mm", {})
        uw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        uh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        sheet_a = uw * uh
        used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
        per_sheet.append(used / sheet_a * 100 if sheet_a > 0 else 0.0)
    min_util = min(per_sheet) if per_sheet else 0.0
    max_util = max(per_sheet) if per_sheet else 0.0
    telemetry = data.get("summary", {}).get("candidate_selection", {})
    return {
        "seed": seed,
        "sheets": len(sols),
        "per_sheet_util": [round(u, 1) for u in per_sheet],
        "min_util": round(min_util, 1),
        "max_util": round(max_util, 1),
        "spread": round(max_util - min_util, 1),
        "time_s": round(elapsed, 1),
        "winner_min_util_pct": telemetry.get("winner_min_sheet_util_pct"),
    }


print(f"Testing min-sheet-util tie-breaker: {SEEDS} seeds")
print(f"{'='*80}")
results = []
for s in range(1, SEEDS + 1):
    res = run_one(s)
    results.append(res)
    ss = " ".join(f"{u:.1f}" for u in res["per_sheet_util"])
    print(f"  seed={s:3d}: sheets={res['sheets']}, util=[{ss}], "
          f"min={res['min_util']:.1f}%, spread={res['spread']:.1f}%, "
          f"time={res['time_s']:.1f}s, telemetry_min={res['winner_min_util_pct']}")

print(f"\n{'='*80}")
print("SUMMARY:")
min_utils = [r["min_util"] for r in results]
spreads = [r["spread"] for r in results]
print(f"  Min utilization across seeds: min={min(min_utils):.1f}%, "
      f"avg={statistics.mean(min_utils):.1f}%, max={max(min_utils):.1f}%")
print(f"  Spread (max-min per layout): min={min(spreads):.1f}%, "
      f"avg={statistics.mean(spreads):.1f}%, max={max(spreads):.1f}%")
above_90 = sum(1 for u in min_utils if u >= 90.0)
print(f"  Seeds where ALL sheets >= 90%: {above_90}/{SEEDS}")
above_89 = sum(1 for u in min_utils if u >= 89.0)
print(f"  Seeds where ALL sheets >= 89%: {above_89}/{SEEDS}")
