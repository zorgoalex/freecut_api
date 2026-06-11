"""Regenerate balanced layouts with kerf-fill SVG corridors collapsed."""
import json, os, urllib.request, time

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
OUT_DIR = "ai_docs/tmp/best_layouts_balanced"

with open(FIXTURE) as f:
    base = json.load(f)

# Use portfolio mode with longer time for best results
base["params"]["time_limit_ms"] = 10000
base["params"]["restarts"] = 5
base["params"]["layout_mode"] = "guillotine"
base["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
base["params"]["include_svg"] = True

best_seeds = [15, 3, 10, 12, 14]

results = []
for seed in best_seeds:
    base["params"]["seed"] = seed
    payload = json.dumps(base).encode()
    req = urllib.request.Request(
        "http://localhost:8088/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    t0 = time.time()
    resp = urllib.request.urlopen(req, timeout=120)
    dt = time.time() - t0
    data = json.loads(resp.read())

    sheets = data.get("summary", {}).get("used_stock_count", 0)
    waste = data.get("summary", {}).get("total_waste_area_mm2", 0)
    svg = data.get("artifacts", {}).get("svg", "") or ""

    # Check per-sheet utilization
    per_sheet = []
    for sol in data.get("solutions", []):
        trim = sol.get("trim_mm", {})
        sw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        sh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        stock_area = sw * sh
        items_area = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
        util = items_area / stock_area * 100 if stock_area > 0 else 0
        per_sheet.append(round(util, 1))

    min_util = min(per_sheet) if per_sheet else 0
    results.append((seed, sheets, waste, min_util, per_sheet, svg, dt))
    print(f"  seed={seed:3d}: sheets={sheets}, waste={waste:.0f}, min_util={min_util:.1f}%, per_sheet={per_sheet}, time={dt:.1f}s")

os.makedirs(OUT_DIR, exist_ok=True)

# Save top-5 by min_util
results.sort(key=lambda r: -r[3])
for rank, (seed, sheets, waste, min_util, per_sheet, svg, dt) in enumerate(results, 1):
    svg_path = os.path.join(OUT_DIR, f"rank_{rank:02d}_minutil_{min_util:.0f}_seed_{seed}.svg")
    with open(svg_path, "w") as f:
        f.write(svg)
    print(f"  Saved: {svg_path}")

print(f"\nDone! Saved {len(results)} layouts with collapsed corridors.")
