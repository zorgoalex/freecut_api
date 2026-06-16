"""V40: Corridor score metric — measures how much waste is in narrow corridors.

For each sheet, flood-fills waste zones (no kerf inflation), then for each zone:
- Computes bbox of the zone
- Computes fill% = zone_area / bbox_area
- If fill% < 0.25 → zone is a "corridor" (narrow, snake-like)
- corridor_score = sum(corridor_zone_areas) / total_waste_area

Ideal layout: corridor_score ≈ 0 (all waste in compact corner rectangles)
Bad layout: corridor_score ≈ 0.5+ (half the waste is in narrow corridors)

Also measures compactness = largest_zone_area / largest_zone_bbox_area
"""

import json
import math
import os
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "ai_docs", "tmp", "v40_corridor_score")
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
        return None


def sheet_metrics(sol, cell=10):
    """Compute per-sheet waste metrics: zones, corridor_score, compactness."""
    trim = sol.get("trim_mm", {})
    sw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
    sh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
    if sw <= 0 or sh <= 0:
        return None
    nx, ny = int(sw // cell), int(sh // cell)
    if nx <= 0 or ny <= 0:
        return None

    occ = [[False] * nx for _ in range(ny)]
    for p in sol.get("placements", []):
        px = p["x_mm"] - trim.get("left", 0)
        py = p["y_mm"] - trim.get("top", 0)
        for j in range(max(0, int(py // cell)), min(ny, int((py + p["height_mm"] - 0.01) // cell) + 1)):
            for i in range(max(0, int(px // cell)), min(nx, int((px + p["width_mm"] - 0.01) // cell) + 1)):
                occ[j][i] = True

    seen = [[False] * nx for _ in range(ny)]
    zones = []
    for j in range(ny):
        for i in range(nx):
            if occ[j][i] or seen[j][i]:
                continue
            stack = [(i, j)]
            seen[j][i] = True
            cells = 0
            min_i, max_i, min_j, max_j = i, i, j, j
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
            bbox_w = (max_i - min_i + 1) * cell
            bbox_h = (max_j - min_j + 1) * cell
            bbox_area = bbox_w * bbox_h
            fill_pct = area / bbox_area if bbox_area > 0 else 1.0
            if area >= 5000:
                zones.append({"area": area, "bbox_area": bbox_area, "fill_pct": fill_pct, "bbox_w": bbox_w, "bbox_h": bbox_h})

    if not zones:
        return {"zones": 0, "total_waste": 0, "corridor_score": 0, "compactness": 0, "largest_frac": 0}

    total_waste = sum(z["area"] for z in zones)
    corridor_area = sum(z["area"] for z in zones if z["fill_pct"] < 0.25)
    corridor_score = corridor_area / total_waste if total_waste > 0 else 0
    largest = max(zones, key=lambda z: z["area"])
    compactness = largest["area"] / largest["bbox_area"] if largest["bbox_area"] > 0 else 0
    largest_frac = largest["area"] / total_waste if total_waste > 0 else 0

    return {
        "zones": len(zones),
        "total_waste": total_waste,
        "corridor_score": round(corridor_score, 4),
        "compactness": round(compactness, 4),
        "largest_frac": round(largest_frac, 4),
    }


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
            resp = call_optimize(req)
            if resp is None:
                print(f"  s{seed}: FAIL", flush=True)
                continue
            pool = resp.get("summary", {}).get("profile_pool") or {}
            wr_v = pool.get("winner_visual_waste_regions", 0)
            gs = resp.get("summary", {}).get("group_shift") or {}
            ns = len(resp.get("solutions", []))
            utils = []
            for sol in resp.get("solutions", []):
                trim = sol.get("trim_mm", {})
                w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
                h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
                used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
                utils.append(used / (w * h) * 100 if w > 0 and h > 0 else 0)
            lead = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else utils[0]

            sheet_ms = [sheet_metrics(sol) for sol in resp.get("solutions", [])]
            sheet_ms = [m for m in sheet_ms if m]
            total_zones = sum(m["zones"] for m in sheet_ms)
            avg_corridor = sum(m["corridor_score"] for m in sheet_ms) / len(sheet_ms) if sheet_ms else 0
            avg_compact = sum(m["compactness"] for m in sheet_ms) / len(sheet_ms) if sheet_ms else 0
            avg_lfrac = sum(m["largest_frac"] for m in sheet_ms) / len(sheet_ms) if sheet_ms else 0

            rows.append({
                "seed": seed, "sheets": ns, "lead": round(lead, 2),
                "wr_v": wr_v, "zones": total_zones,
                "corridor": round(avg_corridor, 4), "compact": round(avg_compact, 4),
                "lfrac": round(avg_lfrac, 4),
                "gs_closed": gs.get("corridor_closed_area_mm2", 0),
            })
            gs_tag = f"gs{gs.get('moves_applied',0)}m" if gs.get("moves_applied", 0) > 0 else ""
            print(f"  s{seed}: {ns}s lead={lead:.2f}% zones={total_zones} corr={avg_corridor:.3f} comp={avg_compact:.3f} lfrac={avg_lfrac:.3f} {gs_tag}", flush=True)

        all_results[tag] = rows
        if rows:
            n = len(rows)
            avg_lead = sum(r["lead"] for r in rows) / n
            avg_zones = sum(r["zones"] for r in rows) / n
            avg_corr = sum(r["corridor"] for r in rows) / n
            avg_comp = sum(r["compact"] for r in rows) / n
            avg_lfrac = sum(r["lfrac"] for r in rows) / n
            le4 = sum(1 for r in rows if r["zones"] <= 4)
            le5 = sum(1 for r in rows if r["zones"] <= 5)
            corr_improved = sum(1 for r in rows if r["corridor"] < 0.15)
            print(f"\n  SUMMARY: lead={avg_lead:.2f}% zones={avg_zones:.1f} corridor={avg_corr:.3f} compact={avg_comp:.4f} lfrac={avg_lfrac:.4f} le4={le4}/{n} le5={le5}/{n} corr<15%={corr_improved}/{n}", flush=True)

    with open(os.path.join(OUT_DIR, "v40_corridor_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)

    # Compare
    b = all_results.get("baseline", [])
    g = all_results.get("gs_on", [])
    if b and g:
        print(f"\n=== COMPARISON ===", flush=True)
        n = min(len(b), len(g))
        corr_improved = sum(1 for i in range(n) if g[i]["corridor"] < b[i]["corridor"] - 0.005)
        corr_worsened = sum(1 for i in range(n) if g[i]["corridor"] > b[i]["corridor"] + 0.005)
        comp_improved = sum(1 for i in range(n) if g[i]["compact"] > b[i]["compact"] + 0.005)
        zones_improved = sum(1 for i in range(n) if g[i]["zones"] < b[i]["zones"])
        zones_worsened = sum(1 for i in range(n) if g[i]["zones"] > b[i]["zones"])
        avg_corr_delta = sum(g[i]["corridor"] - b[i]["corridor"] for i in range(n)) / n
        avg_comp_delta = sum(g[i]["compact"] - b[i]["compact"] for i in range(n)) / n
        print(f"corridor: improved={corr_improved}/{n} worsened={corr_worsened}/{n} avg_delta={avg_corr_delta:+.4f}", flush=True)
        print(f"compactness: improved={comp_improved}/{n} avg_delta={avg_comp_delta:+.4f}", flush=True)
        print(f"zones: improved={zones_improved}/{n} worsened={zones_worsened}/{n}", flush=True)

    print(f"\nSaved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()