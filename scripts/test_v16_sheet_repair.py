"""V16 hypothesis test: post-partition per-sheet repair.

Runs the same heavy varied-4-sheets protocol as the V15 zones-fitness sweep,
but allows a shorter seed count for quick iteration.  The service under test
is expected to include V16 repair code; this script only measures geometry and
saves the best SVG artifacts.
"""

import json
import os
import sys
import time
import urllib.request
from collections import deque

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.environ.get(
    "FREECUT_OUT_DIR",
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v16_sheet_repair"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8088")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "10"))
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
KERF = float(os.environ.get("FREECUT_KERF_MM", "6.0"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = TIME_LIMIT_MS
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
base_req["params"]["portfolio"] = {
    "enabled": True,
    "candidate_count": 5,
    "deadline_ms": TIME_LIMIT_MS,
}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
base_req["params"]["partition"] = {"enabled": True}
if SHEET_BUDGET_MS:
    base_req["params"]["partition"]["sheet_budget_ms"] = SHEET_BUDGET_MS


def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    request = urllib.request.Request(
        f"http://localhost:{PORT}/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=240) as response:
        return json.loads(response.read())


def sheet_geometry(solution):
    trim = solution.get("trim_mm", {})
    width = solution["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
    height = solution["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
    pieces = solution.get("placements", [])
    used = sum(p["width_mm"] * p["height_mm"] for p in pieces)
    util = used / (width * height) * 100.0 if width > 0 and height > 0 else 0.0

    cell = 10.0
    nx, ny = int(width // cell), int(height // cell)
    occ = [[False] * nx for _ in range(ny)]
    for piece in pieces:
        x0, y0 = piece["x_mm"], piece["y_mm"]
        x1, y1 = x0 + piece["width_mm"], y0 + piece["height_mm"]
        for j in range(max(0, int(y0 // cell)), min(ny, int((y1 + cell - 1) // cell))):
            for i in range(max(0, int(x0 // cell)), min(nx, int((x1 + cell - 1) // cell))):
                cx, cy = (i + 0.5) * cell, (j + 0.5) * cell
                if x0 - KERF <= cx <= x1 + KERF and y0 - KERF <= cy <= y1 + KERF:
                    occ[j][i] = True

    seen = [[False] * nx for _ in range(ny)]
    regions = []
    for j in range(ny):
        for i in range(nx):
            if occ[j][i] or seen[j][i]:
                continue
            queue = deque([(i, j)])
            seen[j][i] = True
            cells = []
            while queue:
                ci, cj = queue.popleft()
                cells.append((ci, cj))
                for di, dj in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ni, nj = ci + di, cj + dj
                    if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                        seen[nj][ni] = True
                        queue.append((ni, nj))
            area = len(cells) * cell * cell
            if area >= 5000:
                xs = [c[0] for c in cells]
                ys = [c[1] for c in cells]
                bbox = ((max(xs) - min(xs) + 1) * cell) * ((max(ys) - min(ys) + 1) * cell)
                regions.append((area, area / bbox * 100.0))
    regions.sort(reverse=True)

    lefts = {0.0} | {
        p["x_mm"] + p["width_mm"]
        for p in pieces
        if p["x_mm"] + p["width_mm"] < width
    }
    corner = 0.0
    for left in lefts:
        max_bottom = 0.0
        for piece in pieces:
            if piece["x_mm"] < width and piece["x_mm"] + piece["width_mm"] > left:
                max_bottom = max(max_bottom, piece["y_mm"] + piece["height_mm"])
        corner = max(corner, (width - left) * (height - max_bottom))

    return {
        "util": util,
        "n_regions": len(regions),
        "top_fill": regions[0][1] if regions else 100.0,
        "corner": corner,
    }


def main():
    results = []
    started = time.time()
    for seed in range(1, SEEDS + 1):
        try:
            data = call_optimize(seed)
        except Exception as exc:
            print(f"  seed={seed:2d}: ERROR {exc}", flush=True)
            continue

        solutions = data.get("solutions", [])
        geometry = [sheet_geometry(s) for s in solutions]
        utils = [g["util"] for g in geometry]
        min_util = min(utils) if utils else 0.0
        sorted_utils = sorted(utils, reverse=True)
        lead_util = (
            sum(sorted_utils[:-1]) / max(1, len(sorted_utils) - 1)
            if len(sorted_utils) > 1
            else min_util
        )
        total_regions = sum(g["n_regions"] for g in geometry)
        max_corner = max((g["corner"] for g in geometry), default=0.0)
        sum_corner = sum(g["corner"] for g in geometry)
        ptn = data.get("summary", {}).get("partition") or {}
        row = {
            "seed": seed,
            "sheets": len(solutions),
            "min_util": round(min_util, 2),
            "lead_util": round(lead_util, 2),
            "utils": [round(u, 1) for u in utils],
            "n_waste_regions": total_regions,
            "max_corner_mm2": round(max_corner),
            "sum_corner_mm2": round(sum_corner),
            "partition_applied": ptn.get("applied", False),
            "densest_zones": ptn.get("densest_zones", []),
            "data": data,
        }
        results.append(row)
        mark = "OK " if row["sheets"] == 4 else "5s "
        print(
            f"  seed={seed:2d}: {mark}sheets={row['sheets']}, "
            f"lead={lead_util:5.2f}%, regions={total_regions}, "
            f"max_corner={max_corner / 1e3:.0f}k, "
            f"peel={row['partition_applied']}, zones={row['densest_zones']}",
            flush=True,
        )

    elapsed = time.time() - started
    n = max(1, len(results))
    n_4 = sum(1 for r in results if r["sheets"] == 4)
    avg_lead = sum(r["lead_util"] for r in results) / n
    avg_min = sum(r["min_util"] for r in results) / n
    avg_regions = sum(r["n_waste_regions"] for r in results) / n
    avg_max_corner = sum(r["max_corner_mm2"] for r in results) / n
    n_regions_le_4 = sum(1 for r in results if r["n_waste_regions"] <= 4)
    n_regions_le_5 = sum(1 for r in results if r["n_waste_regions"] <= 5)

    print(f"\nV16 Summary ({len(results)} seeds): elapsed {elapsed:.0f}s", flush=True)
    print(f"  4-sheet rate:                 {n_4}/{len(results)}", flush=True)
    print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%", flush=True)
    print(f"  Avg min util:                 {avg_min:.2f}%", flush=True)
    print(f"  Avg waste regions per layout: {avg_regions:.2f}", flush=True)
    print(f"  Avg max corner rect:          {avg_max_corner / 1e3:.0f}k mm2", flush=True)
    print(f"  Layouts with <=4 regions:     {n_regions_le_4}/{len(results)}", flush=True)
    print(f"  Layouts with <=5 regions:     {n_regions_le_5}/{len(results)}", flush=True)

    ranked = sorted(
        [r for r in results if r["sheets"] == 4],
        key=lambda r: (r["n_waste_regions"], -r["lead_util"]),
    )
    for i, row in enumerate(ranked[:5], start=1):
        svg = row.get("data", {}).get("artifacts", {}).get("svg", "")
        stem = f"rank_{i:02d}_zones{row['n_waste_regions']}_seed_{row['seed']}"
        if svg:
            with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
                f.write(svg)
        with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
            json.dump({k: v for k, v in row.items() if k != "data"}, f, indent=2, ensure_ascii=False)
        print(
            f"  rank {i}: seed={row['seed']}, zones={row['n_waste_regions']}, "
            f"corner={row['max_corner_mm2'] / 1e3:.0f}k, lead={row['lead_util']}%, "
            f"utils={row['utils']}, densest_zones={row['densest_zones']}",
            flush=True,
        )

    with open(os.path.join(OUT_DIR, "v16_sheet_repair_summary.json"), "w", encoding="utf-8") as f:
        json.dump(
            {"results": [{k: v for k, v in r.items() if k != "data"} for r in results]},
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
