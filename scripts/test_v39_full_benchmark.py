"""V39 full benchmark: profile_pool with visual waste regions metric.

Compares baseline (V22-equivalent) with group_shift enabled/disabled,
using the new visual_waste_regions metric (no kerf inflation).

Environment variables:
  FREECUT_PORT  - server port (default 8092)
  FREECUT_SEEDS - number of seeds (default 15)
"""

import json
import os
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.environ.get("FREECUT_OUT_DIR", os.path.join(ROOT, "ai_docs", "tmp", "v39_full_benchmark"))
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
BASE_URL = f"http://127.0.0.1:{PORT}"
SEEDS = int(os.environ.get("FREECUT_SEEDS", "15"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 8000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = False
base_req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 8000}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
base_req["params"]["partition"] = {"enabled": True, "sheet_budget_ms": 15000}
base_req["params"]["profile_pool"] = {
    "enabled": True,
    "zone_penalties": [0.2, 0.3, 0.4, 0.5],
    "fill_penalty": 0.1,
    "max_lead_drop_pp": 0.8,
}

CONFIGS = [
    {"tag": "baseline", "group_shift": None},
    {"tag": "gs_on", "group_shift": {"enabled": True, "min_shift_mm": 5.0, "max_passes": 4}},
]


def call_optimize(req):
    data = json.dumps(req).encode("utf-8")
    r = urllib.request.Request(BASE_URL + "/v1/optimize", data=data, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        print(f"  HTTP {e.code}", flush=True)
        return None


def compute_visual_metrics(solutions, kerf_mm=6, spacing_mm=0):
    cell = 10
    total_zones = 0
    total_waste = 0
    largest_zone = 0
    largest_bbox = 0
    for sol in solutions:
        trim = sol.get("trim_mm", {})
        sw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        sh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        if sw <= 0 or sh <= 0:
            continue
        nx = int(sw // cell)
        ny = int(sh // cell)
        if nx <= 0 or ny <= 0:
            continue
        occ = [[False] * nx for _ in range(ny)]
        for p in sol.get("placements", []):
            px = p["x_mm"] - trim.get("left", 0)
            py = p["y_mm"] - trim.get("top", 0)
            pw = p["width_mm"]
            ph = p["height_mm"]
            i0 = max(0, int(px // cell))
            j0 = max(0, int(py // cell))
            i1 = min(nx - 1, int((px + pw - 0.01) // cell))
            j1 = min(ny - 1, int((py + ph - 0.01) // cell))
            for j in range(j0, j1 + 1):
                for i in range(i0, i1 + 1):
                    occ[j][i] = True
        seen = [[False] * nx for _ in range(ny)]
        for j in range(ny):
            for i in range(nx):
                if occ[j][i] or seen[j][i]:
                    continue
                stack = [(i, j)]
                seen[j][i] = True
                cells = 0
                min_i, max_i = i, i
                min_j, max_j = j, j
                while stack:
                    ci, cj = stack.pop()
                    cells += 1
                    min_i = min(min_i, ci)
                    max_i = max(max_i, ci)
                    min_j = min(min_j, cj)
                    max_j = max(max_j, cj)
                    for di, dj in [(-1, 0), (1, 0), (0, -1), (0, 1)]:
                        ni, nj = ci + di, cj + dj
                        if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                            seen[nj][ni] = True
                            stack.append((ni, nj))
                area = cells * cell * cell
                if area >= 5000:
                    total_zones += 1
                    total_waste += area
                    if area > largest_zone:
                        largest_zone = area
                        bbox_w = (max_i - min_i + 1) * cell
                        bbox_h = (max_j - min_j + 1) * cell
                        largest_bbox = bbox_w * bbox_h
    compact = largest_zone / largest_bbox if largest_bbox > 0 else 0
    lfrac = largest_zone / total_waste if total_waste > 0 else 0
    return {"visual_zones": total_zones, "compactness": round(compact, 4), "largest_frac": round(lfrac, 4)}


def main():
    all_results = {}
    for cfg in CONFIGS:
        tag = cfg["tag"]
        print(f"\n=== {tag} ===", flush=True)
        rows = []
        for seed in range(1, SEEDS + 1):
            req = json.loads(json.dumps(base_req))
            req["params"]["seed"] = seed
            if cfg["group_shift"]:
                req["params"]["group_shift"] = cfg["group_shift"]
            t0 = time.time()
            resp = call_optimize(req)
            elapsed = time.time() - t0
            if resp is None:
                print(f"  s{seed}: FAIL", flush=True)
                continue
            pool = resp.get("summary", {}).get("profile_pool") or {}
            wr_kerf = pool.get("winner_waste_regions", 0)
            wr_visual = pool.get("winner_visual_waste_regions", wr_kerf)
            vis = compute_visual_metrics(resp.get("solutions", []))
            utils = []
            for sol in resp.get("solutions", []):
                trim = sol.get("trim_mm", {})
                w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
                h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
                used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
                utils.append(used / (w * h) * 100 if w > 0 and h > 0 else 0)
            lead = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else utils[0]
            ns = len(resp.get("solutions", []))
            gs = resp.get("summary", {}).get("group_shift") or {}
            row = {
                "seed": seed, "sheets": ns, "lead_util": round(lead, 2),
                "wr_kerf": wr_kerf, "wr_visual": wr_visual,
                "vis_zones": vis["visual_zones"], "compactness": vis["compactness"],
                "largest_frac": vis["largest_frac"],
                "gs_moves": gs.get("moves_applied", 0), "gs_closed": gs.get("corridor_closed_area_mm2", 0),
                "elapsed": round(elapsed, 1),
            }
            rows.append(row)
            print(f"  s{seed}: {ns}s lead={lead:.2f}% wr_k={wr_kerf} wr_v={wr_visual} vz={vis['visual_zones']} comp={vis['compactness']:.3f} lfrac={vis['largest_frac']:.3f} gs={gs.get('moves_applied',0)}m", flush=True)
        all_results[tag] = rows
        if rows:
            n = len(rows)
            avg_lead = sum(r["lead_util"] for r in rows) / n
            avg_wr_k = sum(r["wr_kerf"] for r in rows) / n
            avg_wr_v = sum(r["wr_visual"] for r in rows) / n
            avg_vz = sum(r["vis_zones"] for r in rows) / n
            le4_k = sum(1 for r in rows if r["wr_kerf"] <= 4)
            le4_v = sum(1 for r in rows if r["wr_visual"] <= 4)
            le5_k = sum(1 for r in rows if r["wr_kerf"] <= 5)
            le5_v = sum(1 for r in rows if r["wr_visual"] <= 5)
            avg_compact = sum(r["compactness"] for r in rows) / n
            print(f"  SUMMARY: lead={avg_lead:.2f}% wr_k={avg_wr_k:.1f} wr_v={avg_wr_v:.1f} vz={avg_vz:.1f} le4_k={le4_k}/{n} le4_v={le4_v}/{n} le5_k={le5_k}/{n} le5_v={le5_v}/{n} compact={avg_compact:.4f}", flush=True)

    with open(os.path.join(OUT_DIR, "v39_benchmark_results.json"), "w") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nResults saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()