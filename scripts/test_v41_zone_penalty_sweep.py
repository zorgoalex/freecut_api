"""V41: Test if higher zone_penalties can produce 4-zone layouts for 5-zone seeds.

Baseline (zone_penalties [0.2,0.3,0.4,0.5]) produces 5 zones for 12/20 seeds.
This script tests aggressive zone penalties (0.6, 0.8, 1.0, 1.5, 2.0) to see if any
can push the GA to find 1-zone-per-sheet layouts for those seeds.

For each seed that produces 5 zones at baseline, we also try disabling partition
to see if the non-peeled path finds better layouts.
"""

import json
import os
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "ai_docs", "tmp", "v41_zone_penalty_sweep")
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
BASE_URL = f"http://127.0.0.1:{PORT}"

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 8000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = False

SEEDS_5ZONE = [1, 3, 4, 7, 10, 11, 12, 13, 14, 16, 17, 19]

ZONE_PENALTY_CONFIGS = [
    {"tag": "zp_0.6", "penalties": [0.6]},
    {"tag": "zp_0.8", "penalties": [0.8]},
    {"tag": "zp_1.0", "penalties": [1.0]},
    {"tag": "zp_1.5", "penalties": [1.5]},
    {"tag": "zp_2.0", "penalties": [2.0]},
    {"tag": "zp_0.2_0.3_0.5_0.8_1.2", "penalties": [0.2, 0.3, 0.5, 0.8, 1.2]},
    {"tag": "no_partition", "penalties": None},
]


def call_optimize(req):
    data = json.dumps(req).encode("utf-8")
    r = urllib.request.Request(
        BASE_URL + "/v1/optimize", data=data,
        headers={"Content-Type": "application/json"}, method="POST",
    )
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        return None


def response_visual_waste_regions(resp):
    if not resp or "solutions" not in resp:
        return None
    total = 0
    for sol in resp["solutions"]:
        trim = sol.get("trim_mm", {})
        w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        if w <= 0 or h <= 0:
            continue
        cell = 10
        nx, ny = int(w // cell), int(h // cell)
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
        zones = 0
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
                    zones += 1
        total += zones
    return total


def response_lead_util(resp):
    if not resp or "solutions" not in resp:
        return None
    utils = []
    for sol in resp["solutions"]:
        trim = sol.get("trim_mm", {})
        w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
        if w > 0 and h > 0:
            utils.append(used / (w * h) * 100)
    if not utils:
        return None
    utils.sort(reverse=True)
    return sum(utils[:-1]) / (len(utils) - 1) if len(utils) > 1 else utils[0]


def main():
    all_results = {}
    for cfg in ZONE_PENALTY_CONFIGS:
        tag = cfg["tag"]
        penalties = cfg["penalties"]
        print(f"\n=== {tag} ({len(SEEDS_5ZONE)} 5-zone seeds) ===", flush=True)
        rows = []
        for seed in SEEDS_5ZONE:
            req = json.loads(json.dumps(base_req))
            req["params"]["seed"] = seed
            if penalties is not None:
                req["params"]["profile_pool"] = {
                    "enabled": True,
                    "zone_penalties": penalties,
                    "fill_penalty": 0.1,
                    "max_lead_drop_pp": 0.8,
                }
                req["params"]["partition"] = {
                    "enabled": True,
                    "sheet_budget_ms": 15000,
                }
            else:
                req["params"]["profile_pool"] = {
                    "enabled": True,
                    "zone_penalties": [0.2, 0.3, 0.4, 0.5],
                    "fill_penalty": 0.1,
                    "max_lead_drop_pp": 0.8,
                }
                req["params"]["partition"] = {"enabled": False}

            resp = call_optimize(req)
            if resp is None:
                print(f"  s{seed}: FAIL", flush=True)
                continue

            wr_v = response_visual_waste_regions(resp)
            lead = response_lead_util(resp)
            ns = len(resp.get("solutions", []))

            rows.append({
                "seed": seed, "sheets": ns, "lead": round(lead, 2) if lead else None,
                "wr_v": wr_v, "tag": tag,
            })

            improved = "OK" if wr_v and wr_v <= 4 else ""
            print(f"  s{seed}: {ns}s lead={lead:.2f}% zones={wr_v} {improved}", flush=True)

        all_results[tag] = rows
        if rows:
            n = len(rows)
            avg_lead = sum(r["lead"] for r in rows if r["lead"]) / max(1, sum(1 for r in rows if r["lead"]))
            avg_zones = sum(r["wr_v"] for r in rows if r["wr_v"]) / max(1, sum(1 for r in rows if r["wr_v"]))
            le4 = sum(1 for r in rows if r["wr_v"] and r["wr_v"] <= 4)
            print(f"  SUMMARY: lead={avg_lead:.2f}% zones={avg_zones:.1f} le4={le4}/{n}", flush=True)

    with open(os.path.join(OUT_DIR, "v41_zone_sweep_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()