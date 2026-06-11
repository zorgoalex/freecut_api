"""Quick test: run 10 seeds with guillotine+standard to check sheet penalty effect."""
import json, requests, statistics, time, copy

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"
SEEDS = 10

with open(FIXTURE) as f:
    base_req = json.load(f)

results = []
for seed in range(SEEDS):
    req = copy.deepcopy(base_req)
    req["params"]["seed"] = seed
    req["params"]["layout_mode"] = "guillotine"
    req["params"]["objective"] = "min_waste"
    req["params"]["time_limit_ms"] = 5000
    req["params"]["restarts"] = 5
    # Use best params from sweep
    req["params"]["ga_override"] = {
        "epochs": 300,
        "breed_factor": 0.5,
        "survival_factor": 0.8,
        "top_k_candidates": 3
    }
    
    t0 = time.time()
    r = requests.post(URL, json=req, timeout=60)
    dt = time.time() - t0
    
    if r.status_code != 200:
        print(f"seed={seed}: ERROR {r.status_code} {r.text[:200]}")
        continue
    
    data = r.json()
    summary = data.get("summary", {})
    cs = summary.get("candidate_selection", {})
    
    sheets = summary.get("used_stock_count", 0)
    waste = summary.get("total_waste_area_mm2", 0)
    perim = cs.get("winner_piece_perimeter_mm", 0)
    bbox_void = cs.get("winner_bbox_void_area_mm2", 0)
    
    results.append({
        "seed": seed,
        "sheets": sheets,
        "waste": waste,
        "perim": perim,
        "bbox_void": bbox_void,
        "time": round(dt, 2)
    })
    print(f"seed={seed}: sheets={sheets} waste={waste:.0f} perim={perim:.0f} void={bbox_void:.0f} time={dt:.1f}s")

if not results:
    print("No results!")
    exit(1)

sheets_list = [r["sheets"] for r in results]
avg_sheets = statistics.mean(sheets_list)
avg_waste = statistics.mean([r["waste"] for r in results])
avg_perim = statistics.mean([r["perim"] for r in results])
avg_void = statistics.mean([r["bbox_void"] for r in results])

print(f"\n{'='*60}")
print(f"RESULTS: sheet penalty v1 (alpha=0.15), guillotine+standard")
print(f"{'='*60}")
print(f"Avg sheets:  {avg_sheets:.2f}  (baseline: 4.80)")
print(f"Avg waste:   {avg_waste:.0f} mm2")
print(f"Avg perim:   {avg_perim:.0f} mm")
print(f"Avg void:    {avg_void:.0f} mm2")
print(f"Sheets dist: {sorted(sheets_list)}")
print(f"4-sheet:     {sheets_list.count(4)}/{len(results)}")
print(f"5-sheet:     {sheets_list.count(5)}/{len(results)}")
