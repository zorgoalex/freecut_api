"""V7: verify the heuristic double-placement fix.
Same 30-seed test as V3-V5 to confirm no regression on the main fixture
AND the optimize_valid anomaly is fixed.
"""
import json, os, sys, urllib.request, time

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

OUT_DIR = "ai_docs/tmp/best_layouts_v7"
os.makedirs(OUT_DIR, exist_ok=True)

with open("tests/fixtures/multisheet_varied_4sheets.json") as f:
    base_req = json.load(f)
base_req["params"]["time_limit_ms"] = 10000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
base_req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3

with open("tests/fixtures/optimize_valid.json") as f:
    valid_req = json.load(f)
valid_req["params"]["time_limit_ms"] = 1000
valid_req["params"]["restarts"] = 1
valid_req["params"]["retry_strategy"] = "disabled"
valid_req["params"]["include_svg"] = False


def call(req):
    payload = json.dumps(req).encode()
    r = urllib.request.Request("http://localhost:8088/v1/optimize", data=payload,
        headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(r, timeout=180).read())


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


# Part 1: optimize_valid sanity check (the bug)
print("=== Part 1: optimize_valid (the bug) ===")
req = json.loads(json.dumps(valid_req))
req["params"]["seed"] = 1
data = call(req)
total = sum(len(s["placements"]) for s in data["solutions"])
utils = per_sheet_utils(data)
print("  optimize_valid seed=1: {} placements, util={}, sheets={}".format(
    total, [f'{u:.2f}%' for u in utils], len(data["solutions"])))
print("  EXPECTED: 3 placements, util=[29.15%], sheets=1")
print("  STATUS: {}".format("FIXED" if total == 3 else "STILL BROKEN"))

# Part 2: 30-seed main fixture, ensure no regression vs V5
print("\n=== Part 2: multisheet_varied_4sheets 30 seeds ===")
results = []
t_start = time.time()
for seed in range(1, 31):
    try:
        req = json.loads(json.dumps(base_req))
        req["params"]["seed"] = seed
        data = call(req)
    except Exception as e:
        print(f"  seed={seed}: ERROR {e}")
        continue
    n_sheets = len(data["solutions"])
    svg = data.get("artifacts", {}).get("svg", "") or ""
    utils = per_sheet_utils(data)
    min_util = min(utils) if utils else 0.0
    range_util = max(utils) - min(utils) if utils else 0.0
    all_above_90 = all(u >= 90.0 for u in utils)
    sel = data.get("summary", {}).get("candidate_selection", {}) or {}
    max_edge_gap_mm = sel.get("winner_max_edge_gap_mm", 0.0)
    results.append({
        "seed": seed, "sheets": n_sheets, "min_util": min_util,
        "range": range_util, "all_above_90": all_above_90,
        "edge_gap": max_edge_gap_mm, "data": data,
    })
elapsed = time.time() - t_start
print(f"  Elapsed: {elapsed:.1f}s")

n_4 = sum(1 for r in results if r["sheets"] == 4)
n_4_90 = sum(1 for r in results if r["sheets"] == 4 and r["all_above_90"])
n_4_91 = sum(1 for r in results if r["sheets"] == 4 and r["min_util"] >= 91.0)
avg_min = sum(r["min_util"] for r in results) / max(1, len(results))
avg_range = sum(r["range"] for r in results) / max(1, len(results))
avg_edge = sum(r["edge_gap"] for r in results) / max(1, len(results))

print(f"\nV7 summary (30 seeds, multisheet_varied_4sheets):")
print(f"  4-sheet rate:                       {n_4}/{len(results)}")
print(f"  4-sheet AND all >= 90%:             {n_4_90}/{len(results)} ({100*n_4_90/len(results):.1f}%)")
print(f"  4-sheet AND min >= 91%:             {n_4_91}/{len(results)}")
print(f"  Avg min_util:                       {avg_min:.2f}%")
print(f"  Avg range:                          {avg_range:.2f}%")
print(f"  Avg max_edge_gap:                   {avg_edge:.1f} mm")

print(f"\nV5 baseline (no heuristic fix):")
print(f"  4-sheet rate:                       30/30 (100%)")
print(f"  4-sheet AND all >= 90%:             21/30 (70.0%)")
print(f"  Avg min_util:                       90.39%")

# Save best
ranked = sorted([r for r in results if r["sheets"] == 4 and r["all_above_90"]],
               key=lambda r: (-r["min_util"], r["range"]))
for i, r in enumerate(ranked[:5], start=1):
    svg = r["data"].get("artifacts", {}).get("svg", "") or ""
    svg_path = os.path.join(OUT_DIR, f"rank_{i:02d}_minutil_{int(r['min_util']*100):04d}_seed_{r['seed']}.svg")
    with open(svg_path, "w") as f:
        f.write(svg)
    print(f"  rank {i}: seed={r['seed']}, min={r['min_util']}%, range={r['range']}%")

with open(os.path.join(OUT_DIR, "v7_summary.json"), "w") as f:
    json.dump({
        "results": [{k: v for k, v in r.items() if k != "data"} for r in results],
        "summary": {
            "n_4_sheet": n_4,
            "n_4_sheet_all_above_90": n_4_90,
            "n_4_sheet_min_above_91": n_4_91,
            "avg_min_util": round(avg_min, 2),
            "avg_range": round(avg_range, 2),
            "avg_max_edge_gap_mm": round(avg_edge, 1),
        },
    }, f, indent=2)
print(f"\nSummary saved to {OUT_DIR}/v7_summary.json")
