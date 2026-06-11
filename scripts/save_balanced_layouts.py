"""Collect zero-void layouts and verify internal_void metric."""
import json, requests, time, os, glob

URL = "http://localhost:8088/v1/optimize"
FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
OUT_DIR = "ai_docs/tmp/best_layouts_balanced"
SEEDS = 30
RETRY_MAX = 3
SHEET_AREA = (2070 - 20) * (2800 - 20)

with open(FIXTURE) as f:
    base_req = json.load(f)


def compute_internal_void(sol):
    """Compute internal void using 5mm occupancy grid + flood fill."""
    trim = sol.get("trim_mm", {})
    sw = sol["width_mm"]
    sh = sol["height_mm"]
    tl, tr = trim.get("left", 0), trim.get("right", 0)
    tt, tb = trim.get("top", 0), trim.get("bottom", 0)
    RES = 5  # mm per cell
    cols = int((sw - tl - tr) / RES)
    rows = int((sh - tt - tb) / RES)
    grid = [[False] * cols for _ in range(rows)]
    for p in sol.get("placements", []):
        x0 = max(0, int((p["x_mm"] - tl) / RES))
        y0 = max(0, int((p["y_mm"] - tt) / RES))
        x1 = min(cols, int((p["x_mm"] + p["width_mm"] - tl) / RES))
        y1 = min(rows, int((p["y_mm"] + p["height_mm"] - tt) / RES))
        for y in range(y0, y1):
            for x in range(x0, x1):
                grid[y][x] = True
    # Flood fill from edges
    from collections import deque
    visited = [[False] * cols for _ in range(rows)]
    q = deque()
    for y in range(rows):
        for x in [0, cols - 1]:
            if not grid[y][x] and not visited[y][x]:
                visited[y][x] = True
                q.append((y, x))
    for x in range(cols):
        for y in [0, rows - 1]:
            if not grid[y][x] and not visited[y][x]:
                visited[y][x] = True
                q.append((y, x))
    while q:
        cy, cx = q.popleft()
        for dy, dx in [(-1, 0), (1, 0), (0, -1), (0, 1)]:
            ny, nx = cy + dy, cx + dx
            if 0 <= ny < rows and 0 <= nx < cols and not grid[ny][nx] and not visited[ny][nx]:
                visited[ny][nx] = True
                q.append((ny, nx))
    total_empty = sum(1 for y in range(rows) for x in range(cols) if not grid[y][x])
    void_cells = sum(1 for y in range(rows) for x in range(cols) if not grid[y][x] and not visited[y][x])
    return void_cells / total_empty if total_empty > 0 else 0.0


def run_one(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["time_limit_ms"] = 10000
    req["params"]["restarts"] = 5
    req["params"]["layout_mode"] = "guillotine"
    req["params"]["seed"] = seed
    req["params"]["include_svg"] = True
    req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}
    r = requests.post(URL, json=req, timeout=30)
    return r.json()


os.makedirs(OUT_DIR, exist_ok=True)
results = []

print(f"Collecting balanced layouts with zero-void verification...")
for s in range(1, SEEDS + 1):
    for retry in range(RETRY_MAX):
        data = run_one(s + retry * 100)
        sols = data.get("solutions", [])
        if len(sols) != 4:
            continue
        per_sheet = []
        iv_vals = []
        for sol in sols:
            trim = sol.get("trim_mm", {})
            uw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
            uh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
            sa = uw * uh
            used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
            per_sheet.append(used / sa * 100)
            iv_vals.append(compute_internal_void(sol))
        min_util = min(per_sheet)
        max_iv = max(iv_vals)
        total_pcs = sum(len(sol.get("placements", [])) for sol in sols)
        if total_pcs == 40 and max_iv < 0.01:
            waste = data.get("summary", {}).get("total_waste_area_mm2", 0)
            results.append({
                "seed": s, "retry": retry, "sheets": 4,
                "min_util": round(min_util, 1),
                "max_iv": round(max_iv, 4),
                "waste": waste,
                "per_sheet": [round(u, 1) for u in per_sheet],
                "svg": data.get("artifacts", {}).get("svg", ""),
                "solutions": sols,
            })
            status = "OK"
            break
        else:
            status = f"iv={max_iv:.3f}" if total_pcs == 40 else f"pcs={total_pcs}"
    print(f"  seed={s:3d}: {status}, min_util={min_util:.1f}%, "
          f"per_sheet={[round(u,1) for u in per_sheet]}")

print(f"\n{'='*80}")
print(f"Total zero-void 4-sheet layouts: {len(results)}/{SEEDS}")
if results:
    min_utils = [r["min_util"] for r in results]
    print(f"Min util: avg={sum(min_utils)/len(min_utils):.1f}%, "
          f"range=[{min(min_utils):.1f}%, {max(min_utils):.1f}%]")
    above_90 = sum(1 for u in min_utils if u >= 90.0)
    print(f"All sheets >= 90%: {above_90}/{len(results)}")

    # Save top 5 by min_util desc
    results.sort(key=lambda r: (-r["min_util"], r["waste"]))
    for i, r in enumerate(results[:5]):
        rank = i + 1
        ss = " ".join(f"{u:.1f}" for u in r["per_sheet"])
        print(f"  rank_{rank:02d}: seed={r['seed']}, min_util={r['min_util']:.1f}%, "
              f"util=[{ss}]")
        if r["svg"]:
            with open(os.path.join(OUT_DIR, f"rank_{rank:02d}_minutil_{r['min_util']:.0f}_seed_{r['seed']}.svg"), "w") as f:
                f.write(r["svg"])
        with open(os.path.join(OUT_DIR, f"rank_{rank:02d}_minutil_{r['min_util']:.0f}_seed_{r['seed']}.json"), "w") as f:
            json.dump({"seed": r["seed"], "sheets": r["sheets"],
                       "min_util": r["min_util"], "waste": r["waste"],
                       "per_sheet_util": r["per_sheet"],
                       "solutions": r["solutions"]}, f, indent=2)
    print(f"\nSaved top-5 to {OUT_DIR}/")
