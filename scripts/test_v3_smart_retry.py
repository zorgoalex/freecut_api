"""V3 hypothesis test: service-level smart retry.

The V3 service handles retries internally based on a fault classifier.  This
script just sends 30 single-shot requests (no client-side retry loop) and
collects the results + retry telemetry from the response.

Compares V3 vs V2 from ai_docs/tmp/best_layouts_v2/v2_summary.json.
"""
import json, os, sys, urllib.request, time

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

OUT_DIR = "ai_docs/tmp/best_layouts_v3"
os.makedirs(OUT_DIR, exist_ok=True)

with open("tests/fixtures/multisheet_varied_4sheets.json") as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 10000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
# V3: don't ask for retries from the client side - the service does it.
base_req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3


def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    r = urllib.request.Request(
        "http://localhost:8088/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    resp = urllib.request.urlopen(r, timeout=180)
    return json.loads(resp.read())


def per_sheet_utils(data):
    utils = []
    for sol in data.get("solutions", []):
        trim = sol.get("trim_mm", {})
        uw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        uh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        sheet_a = uw * uh
        pieces = sol.get("placements", [])
        used = sum(p["width_mm"] * p["height_mm"] for p in pieces)
        utils.append(used / sheet_a * 100.0 if sheet_a > 0 else 0.0)
    return utils


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
    svg = data.get("artifacts", {}).get("svg", "") or ""
    sel = data.get("summary", {}).get("candidate_selection", {})
    retry = data.get("summary", {}).get("retry", {}) or {}
    waste_mm2 = data.get("summary", {}).get("total_waste_area_mm2", 0)
    utils = per_sheet_utils(data)
    min_util = min(utils) if utils else 0.0
    range_util = max(utils) - min(utils) if utils else 0.0
    all_above_90 = all(u >= 90.0 for u in utils)
    max_edge_gap_mm = sel.get("winner_max_edge_gap_mm", 0.0)
    spread_pct = sel.get("winner_sheet_util_spread_pct", 0.0)
    n_attempts = retry.get("attempts", 1)
    n_retries = retry.get("retries", 0)
    strategies = retry.get("strategies", [])
    initial_failure = retry.get("initial_failure", "")

    results.append({
        "seed": seed,
        "sheets": n_sheets,
        "min_util": round(min_util, 2),
        "range_util": round(range_util, 2),
        "all_above_90": all_above_90,
        "max_edge_gap_mm": round(max_edge_gap_mm, 1),
        "spread_pct": round(spread_pct, 2),
        "waste_mm2": waste_mm2,
        "attempts": n_attempts,
        "retries": n_retries,
        "strategies": strategies,
        "initial_failure": initial_failure,
        "svg_len": len(svg),
        "data": data,
    })
    mark = "OK " if n_sheets == 4 and all_above_90 else ("~~ " if n_sheets == 4 else "5s ")
    strat = "/".join(strategies) if strategies else "none"
    print(f"  seed={seed:2d}: {mark}sheets={n_sheets}, min_util={min_util:5.2f}%, "
          f"range={range_util:4.2f}%, max_edge={max_edge_gap_mm:5.1f}mm, "
          f"attempts={n_attempts}, fail={initial_failure or 'none'}, strat={strat}")

elapsed = time.time() - t_start
print(f"\nElapsed: {elapsed:.1f}s")

n_4 = sum(1 for r in results if r["sheets"] == 4)
n_4_90 = sum(1 for r in results if r["sheets"] == 4 and r["all_above_90"])
n_4_91 = sum(1 for r in results if r["sheets"] == 4 and r["min_util"] >= 91.0)
n_4_92 = sum(1 for r in results if r["sheets"] == 4 and r["min_util"] >= 92.0)
avg_min = sum(r["min_util"] for r in results) / max(1, len(results))
avg_range = sum(r["range_util"] for r in results) / max(1, len(results))
avg_edge = sum(r["max_edge_gap_mm"] for r in results) / max(1, len(results))
avg_spread = sum(r["spread_pct"] for r in results) / max(1, len(results))
n_4_min_4 = sum(1 for r in results if r["sheets"] == 4 and r["range_util"] <= 4.0)
n_4_edge_50 = sum(1 for r in results if r["sheets"] == 4 and r["max_edge_gap_mm"] <= 50.0)
n_4_edge_30 = sum(1 for r in results if r["sheets"] == 4 and r["max_edge_gap_mm"] <= 30.0)
n_4_spread_2 = sum(1 for r in results if r["sheets"] == 4 and r["spread_pct"] <= 2.0)
n_retried = sum(1 for r in results if r["retries"] > 0)
avg_attempts = sum(r["attempts"] for r in results) / max(1, len(results))

print(f"\nV3 Summary (30 seeds, service-level smart retry, 3 attempts max):")
print(f"  4-sheet rate:                       {n_4}/{len(results)} ({100*n_4/len(results):.1f}%)")
print(f"  4-sheet AND all >= 90% util:        {n_4_90}/{len(results)} ({100*n_4_90/len(results):.1f}%)")
print(f"  4-sheet AND min_util >= 91%:        {n_4_91}/{len(results)}")
print(f"  4-sheet AND min_util >= 92%:        {n_4_92}/{len(results)}")
print(f"  4-sheet AND range  <= 4.0%:         {n_4_min_4}/{len(results)}")
print(f"  4-sheet AND max_edge_gap <= 50mm:   {n_4_edge_50}/{len(results)}")
print(f"  4-sheet AND max_edge_gap <= 30mm:   {n_4_edge_30}/{len(results)}")
print(f"  4-sheet AND spread    <= 2.0%:      {n_4_spread_2}/{len(results)}")
print(f"  Avg min_util:                       {avg_min:.2f}%")
print(f"  Avg range:                          {avg_range:.2f}%")
print(f"  Avg max_edge_gap:                   {avg_edge:.1f} mm")
print(f"  Avg spread:                         {avg_spread:.2f}%")
print(f"  Seeds that needed retry:            {n_retried}/{len(results)}")
print(f"  Avg attempts per seed:              {avg_attempts:.2f}")

print(f"\nV2 baseline (no service retry, client retry loop, from v2_summary.json):")
print(f"  4-sheet rate:                       30/30 (100%)")
print(f"  4-sheet AND all >= 90% util:        20/30 (66.7%)")
print(f"  Avg min_util:                       90.20%")
print(f"  Avg max_edge_gap:                   29.6 mm")
print(f"  Avg spread:                         1.49%")
print(f"  4-sheet AND edge<=30mm:             12/30")

# Save top-5 SVGs
ranked = sorted(
    [r for r in results if r["sheets"] == 4 and r["all_above_90"]],
    key=lambda r: (-r["min_util"], r["range_util"]),
)
for i, r in enumerate(ranked[:5], start=1):
    svg = r["data"].get("artifacts", {}).get("svg", "") or ""
    svg_path = os.path.join(OUT_DIR, f"rank_{i:02d}_minutil_{int(r['min_util']*100):04d}_seed_{r['seed']}.svg")
    with open(svg_path, "w") as f:
        f.write(svg)
    json_path = os.path.join(OUT_DIR, f"rank_{i:02d}_minutil_{int(r['min_util']*100):04d}_seed_{r['seed']}.json")
    summary = {k: v for k, v in r["data"].items() if k != "artifacts" or True}
    if "artifacts" in summary and "svg" in summary["artifacts"]:
        summary["artifacts"] = {k: v for k, v in summary["artifacts"].items() if k != "svg"}
    with open(json_path, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"  rank {i}: seed={r['seed']}, min_util={r['min_util']}%, "
          f"range={r['range_util']}%, edge={r['max_edge_gap_mm']}mm, spread={r['spread_pct']}%, "
          f"attempts={r['attempts']}, strat={'/'.join(r['strategies']) or 'none'}")

with open(os.path.join(OUT_DIR, "v3_summary.json"), "w") as f:
    summary = {
        "results": [{k: v for k, v in r.items() if k != "data"} for r in results],
        "summary": {
            "n_seeds": len(results),
            "n_4_sheet": n_4,
            "n_4_sheet_all_above_90": n_4_90,
            "n_4_sheet_min_above_91": n_4_91,
            "n_4_sheet_min_above_92": n_4_92,
            "n_4_sheet_range_le_4": n_4_min_4,
            "n_4_sheet_edge_le_50": n_4_edge_50,
            "n_4_sheet_edge_le_30": n_4_edge_30,
            "n_4_sheet_spread_le_2": n_4_spread_2,
            "avg_min_util": round(avg_min, 2),
            "avg_range": round(avg_range, 2),
            "avg_max_edge_gap_mm": round(avg_edge, 1),
            "avg_spread_pct": round(avg_spread, 2),
            "n_seeds_retried": n_retried,
            "avg_attempts": round(avg_attempts, 2),
        },
    }
    json.dump(summary, f, indent=2)
print(f"\nSummary saved to {OUT_DIR}/v3_summary.json")
