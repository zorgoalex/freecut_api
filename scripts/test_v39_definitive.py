"""V39 definitive benchmark: 30 seeds, baseline vs group_shift, visual metrics.

Compares:
  1. baseline: profile_pool, no group_shift
  2. gs_on:    profile_pool + group_shift

Records winner_visual_waste_regions from API (no kerf inflation)
and computes visual waste metrics from placements.
"""

import json
import os
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.environ.get("FREECUT_OUT_DIR", os.path.join(ROOT, "ai_docs", "tmp", "v39_definitive"))
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
BASE_URL = f"http://127.0.0.1:{PORT}"
SEEDS = int(os.environ.get("FREECUT_SEEDS", "30"))

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
    "enabled": True, "zone_penalties": [0.2, 0.3, 0.4, 0.5],
    "fill_penalty": 0.1, "max_lead_drop_pp": 0.8,
}

CONFIGS = [
    {"tag": "baseline", "gs": None},
    {"tag": "gs_on", "gs": {"enabled": True, "min_shift_mm": 5.0, "max_passes": 4}},
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


def vis_zones(solutions, cell=10):
    total = 0
    for sol in solutions:
        trim = sol.get("trim_mm", {})
        sw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        sh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        if sw <= 0 or sh <= 0:
            continue
        nx, ny = int(sw // cell), int(sh // cell)
        if nx <= 0 or ny <= 0:
            continue
        occ = [[False] * nx for _ in range(ny)]
        for p in sol.get("placements", []):
            px = p["x_mm"] - trim.get("left", 0)
            py = p["y_mm"] - trim.get("top", 0)
            for j in range(max(0, int(py // cell)), min(ny, int((py + p["height_mm"] - 0.01) // cell) + 1)):
                for i in range(max(0, int(px // cell)), min(nx, int((px + p["width_mm"] - 0.01) // cell) + 1)):
                    occ[j][i] = True
        seen = [[False] * nx for _ in range(ny)]
        for j in range(ny):
            for i in range(nx):
                if occ[j][i] or seen[j][i]:
                    continue
                stack = [(i, j)]
                seen[j][i] = True
                cells = 0
                while stack:
                    ci, cj = stack.pop()
                    cells += 1
                    for di, dj in [(-1, 0), (1, 0), (0, -1), (0, 1)]:
                        ni, nj = ci + di, cj + dj
                        if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                            seen[nj][ni] = True
                            stack.append((ni, nj))
                if cells >= 50:
                    total += 1
    return total


def main():
    all_results = {}
    for cfg in CONFIGS:
        tag = cfg["tag"]
        print(f"\n=== {tag} ({SEEDS} seeds) ===", flush=True)
        rows = []
        for seed in range(1, SEEDS + 1):
            req = json.loads(json.dumps(base_req))
            req["params"]["seed"] = seed
            if cfg["gs"]:
                req["params"]["group_shift"] = cfg["gs"]
            t0 = time.time()
            resp = call_optimize(req)
            elapsed = time.time() - t0
            if resp is None:
                print(f"  s{seed}: FAIL", flush=True)
                continue
            pool = resp.get("summary", {}).get("profile_pool") or {}
            wr_k = pool.get("winner_waste_regions", 0)
            wr_v_api = pool.get("winner_visual_waste_regions", wr_k)
            vis_z = vis_zones(resp.get("solutions", []))
            ns = len(resp.get("solutions", []))
            utils = []
            for sol in resp.get("solutions", []):
                trim = sol.get("trim_mm", {})
                w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
                h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
                used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
                utils.append(used / (w * h) * 100 if w > 0 and h > 0 else 0)
            lead = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else utils[0]
            gs = resp.get("summary", {}).get("group_shift") or {}
            rows.append({
                "seed": seed, "sheets": ns, "lead": round(lead, 2),
                "wr_kerf": wr_k, "wr_visual_api": wr_v_api, "vis_z": vis_z,
                "gs_moves": gs.get("moves_applied", 0),
                "gs_closed": gs.get("corridor_closed_area_mm2", 0),
                "t": round(elapsed, 1),
            })
            gs_tag = f"gs{gs.get('moves_applied',0)}m" if gs.get("moves_applied", 0) > 0 else ""
            print(f"  s{seed}: {ns}s lead={lead:.2f}% wr_k={wr_k} wr_v={wr_v_api} vz={vis_z} {gs_tag}", flush=True)

        all_results[tag] = rows
        if rows:
            n = len(rows)
            avg_lead = sum(r["lead"] for r in rows) / n
            avg_wr_k = sum(r["wr_kerf"] for r in rows) / n
            avg_wr_v = sum(r["wr_visual_api"] for r in rows) / n
            avg_vz = sum(r["vis_z"] for r in rows) / n
            le4_k = sum(1 for r in rows if r["wr_kerf"] <= 4)
            le4_v = sum(1 for r in rows if r["wr_visual_api"] <= 4)
            le5_k = sum(1 for r in rows if r["wr_kerf"] <= 5)
            le5_v = sum(1 for r in rows if r["wr_visual_api"] <= 5)
            le4_vz = sum(1 for r in rows if r["vis_z"] <= 4)
            le5_vz = sum(1 for r in rows if r["vis_z"] <= 5)
            print(f"\n  SUMMARY: lead={avg_lead:.2f}% wr_k={avg_wr_k:.1f} wr_v={avg_wr_v:.1f} vz={avg_vz:.1f} le4_k={le4_k}/{n} le4_v={le4_v}/{n} le5_v={le5_v}/{n} le4_vz={le4_vz}/{n} le5_vz={le5_vz}/{n}", flush=True)

    with open(os.path.join(OUT_DIR, "v39_definitive_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()