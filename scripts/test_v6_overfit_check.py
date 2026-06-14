"""V6 overfit-check: run the current V1+V2+V3+V5 config on multiple
fixtures to verify the improvements generalise beyond
multisheet_varied_4sheets.json.

For each fixture, run 5 seeds, save the best layout (by min_util) to
ai_docs/tmp/best_layouts_v6/<fixture>/.
"""
import json, os, sys, urllib.request, time

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

OUT_DIR = "ai_docs/tmp/best_layouts_v6"
os.makedirs(OUT_DIR, exist_ok=True)

FIXTURES = [
    ("multisheet_varied_4sheets", "tests/fixtures/multisheet_varied_4sheets.json"),
    ("multisheet_oversized",     "tests/fixtures/multisheet_oversized.json"),
    ("multisheet_qty_limit",     "tests/fixtures/multisheet_qty_limit.json"),
    ("optimize_valid",           "tests/fixtures/optimize_valid.json"),
]

# Use the same V1+V2+V3+V5 config for all fixtures.
def make_request(base_path, seed):
    with open(base_path) as f:
        req = json.load(f)
    req["params"]["seed"] = seed
    # Normalise: use the V1+V2+V3+V5 time budget regardless of what the
    # fixture asks for.  Several fixtures have time_limit_ms=1000 which is
    # too short for the V3 smart retry (3 attempts x budget).  We want
    # a fair head-to-head, not "fixture-budget vs V1+V2+V3+V5".
    req["params"]["time_limit_ms"] = 10000
    req["params"]["restarts"] = 5
    req["params"]["include_svg"] = True
    # Smart retry + portfolio
    req["params"]["portfolio"] = {
        "enabled": True,
        "candidate_count": 5,
        "deadline_ms": 10000,
    }
    req["params"]["retry_strategy"] = "smart"
    req["params"]["max_retry_attempts"] = 3
    return req


def call(req):
    payload = json.dumps(req).encode()
    r = urllib.request.Request(
        "http://localhost:8088/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    resp = urllib.request.urlopen(r, timeout=120)
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


SEEDS = [1, 5, 10, 15, 25]

per_fixture = {}

t_start = time.time()
for fixture_name, path in FIXTURES:
    print(f"\n{'='*60}")
    print(f"Fixture: {fixture_name}")
    print(f"{'='*60}")
    fixture_dir = os.path.join(OUT_DIR, fixture_name)
    os.makedirs(fixture_dir, exist_ok=True)
    results = []
    for seed in SEEDS:
        try:
            req = make_request(path, seed)
            data = call(req)
        except Exception as e:
            print(f"  seed={seed}: ERROR {e}")
            continue
        sols = data.get("solutions", [])
        n_sheets = len(sols)
        utils = per_sheet_utils(data)
        min_util = min(utils) if utils else 0.0
        range_util = max(utils) - min(utils) if utils else 0.0
        all_above_90 = all(u >= 90.0 for u in utils) if utils else False
        sel = data.get("summary", {}).get("candidate_selection", {}) or {}
        max_edge_gap_mm = sel.get("winner_max_edge_gap_mm", 0.0)
        spread_pct = sel.get("winner_sheet_util_spread_pct", 0.0)
        retry = data.get("summary", {}).get("retry", {}) or {}
        waste = data.get("summary", {}).get("total_waste_area_mm2", 0.0)
        total_pieces = sum(len(s.get("placements", [])) for s in sols)
        unplaced = data.get("summary", {}).get("used_stock_count", 0)
        strat = "/".join(retry.get("strategies", [])) or "none"
        mark = "OK " if all_above_90 and n_sheets > 0 else ("~~ " if n_sheets > 0 else "5s ")
        print(f"  seed={seed:2d}: {mark}sheets={n_sheets}, util={[f'{u:.1f}' for u in utils]}, "
              f"min={min_util:5.2f}%, range={range_util:4.2f}%, edge={max_edge_gap_mm:5.1f}mm, "
              f"spread={spread_pct:.2f}%, attempts={retry.get('attempts', 1)}, strat={strat}")
        results.append({
            "seed": seed, "n_sheets": n_sheets, "min_util": min_util,
            "range": range_util, "all_above_90": all_above_90,
            "edge_gap": max_edge_gap_mm, "spread": spread_pct,
            "waste": waste, "total_pieces": total_pieces,
            "data": data,
        })
    if results:
        n_ok = sum(1 for r in results if r["all_above_90"] and r["n_sheets"] > 0)
        n_4 = sum(1 for r in results if r["n_sheets"] > 0)
        avg_min = sum(r["min_util"] for r in results) / len(results)
        avg_range = sum(r["range"] for r in results) / len(results)
        avg_edge = sum(r["edge_gap"] for r in results) / len(results)
        print(f"  ---- {fixture_name}: {n_ok}/{len(results)} OK (all>=90%), {n_4}/{len(results)} non-empty, "
              f"avg_min={avg_min:.2f}%, avg_range={avg_range:.2f}%, avg_edge={avg_edge:.1f}mm")
        # Save the best layout
        best = max(results, key=lambda r: (r["min_util"], -r["range"]))
        if best["n_sheets"] > 0:
            svg = best["data"].get("artifacts", {}).get("svg", "") or ""
            svg_path = os.path.join(fixture_dir, f"best_seed{best['seed']}_min{int(best['min_util']*100):04d}.svg")
            with open(svg_path, "w") as f:
                f.write(svg)
            print(f"  Best layout saved: {svg_path}")
        per_fixture[fixture_name] = {
            "results": [{k: v for k, v in r.items() if k != "data"} for r in results],
            "summary": {
                "n_seeds": len(results),
                "n_ok": n_ok,
                "n_non_empty": n_4,
                "avg_min_util": round(avg_min, 2),
                "avg_range": round(avg_range, 2),
                "avg_edge_gap": round(avg_edge, 1),
            },
        }

elapsed = time.time() - t_start
print(f"\n{'='*60}")
print(f"Total elapsed: {elapsed:.1f}s")
print(f"{'='*60}")

# Print cross-fixture summary
print("\nCross-fixture summary:")
print(f"{'Fixture':<32} {'Sheets':>8} {'all>=90%':>10} {'avg_min':>9} {'avg_range':>10} {'avg_edge':>10}")
for name, info in per_fixture.items():
    s = info["summary"]
    print(f"{name:<32} {s['n_non_empty']}/{s['n_seeds']:<6} {s['n_ok']}/{s['n_seeds']:<8} "
          f"{s['avg_min_util']:>7.2f}% {s['avg_range']:>8.2f}% {s['avg_edge_gap']:>8.1f}mm")

with open(os.path.join(OUT_DIR, "v6_summary.json"), "w") as f:
    json.dump(per_fixture, f, indent=2)
print(f"\nSummary saved to {OUT_DIR}/v6_summary.json")
