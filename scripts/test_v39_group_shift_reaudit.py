"""V39-reaudit: measure group_shift before/after with MULTIPLE metrics.

Key question: does group_shift actually improve layout quality?
Previous evaluation used only waste_region_count (flood-fill with kerf
inflation, threshold 5000mm²). This may miss real improvements.

New metrics:
1. waste_region_count (existing, with kerf inflation)
2. waste_region_count_no_inflate (without kerf inflation — matches visual)
3. largest_zone_fraction (largest waste zone area / total waste area)
4. waste_compactness (largest zone area / its bbox area)
5. corridor_area (group_shift opportunity score)

Runs each seed TWICE:
  Run A: group_shift disabled → "before" metrics
  Run B: group_shift enabled → "after" metrics
Compares per-seed.
"""

import json
import math
import os
import sys
import time
import urllib.request

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "ai_docs", "tmp", "v39_group_shift_reaudit")
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
BASE_URL = f"http://127.0.0.1:{PORT}"
SEEDS = int(os.environ.get("FREECUT_SEEDS", "15"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 8000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
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


def call_optimize(req):
    data = json.dumps(req).encode("utf-8")
    r = urllib.request.Request(
        f"{BASE_URL}/v1/optimize",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        print(f"  HTTP {e.code}", flush=True)
        return None


def sheet_utils(response):
    out = []
    for s in response.get("solutions", []):
        trim = s.get("trim_mm", {})
        w = s["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = s["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(p["width_mm"] * p["height_mm"] for p in s.get("placements", []))
        out.append({"w": w, "h": h, "used": used, "util": used / (w * h) * 100 if w > 0 and h > 0 else 0})
    return out


def waste_regions_from_response(response):
    pool = response.get("summary", {}).get("profile_pool") or {}
    return pool.get("winner_waste_regions", 0)


def compute_visual_waste_metrics(solutions, kerf_mm=6.0, spacing_mm=0.0):
    """Compute waste metrics WITHOUT kerf inflation (matches visual)."""
    cell = 10  # 10mm grid
    gap = kerf_mm + spacing_mm
    total_zones = 0
    total_waste_area = 0.0
    largest_zone_area = 0.0
    largest_zone_bbox_area = 0.0
    per_sheet = []

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

        placements = sol.get("placements", [])
        for p in placements:
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

        # Flood fill WITHOUT kerf inflation
        seen = [[False] * nx for _ in range(ny)]
        sheet_zones = 0
        sheet_waste = 0.0
        sheet_largest = 0.0
        sheet_largest_bbox = 0.0

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

                area = cells * cell * cell  # mm²
                if area >= 5000:  # same threshold as original
                    sheet_zones += 1
                    sheet_waste += area
                    if area > sheet_largest:
                        sheet_largest = area
                        bbox_w = (max_i - min_i + 1) * cell
                        bbox_h = (max_j - min_j + 1) * cell
                        sheet_largest_bbox = bbox_w * bbox_h

        total_zones += sheet_zones
        total_waste_area += sheet_waste
        if sheet_largest > largest_zone_area:
            largest_zone_area = sheet_largest
            largest_zone_bbox_area = sheet_largest_bbox

        per_sheet.append({
            "zones": sheet_zones,
            "waste_mm2": sheet_waste,
            "largest_zone_mm2": sheet_largest,
        })

    compactness = largest_zone_area / largest_zone_bbox_area if largest_zone_bbox_area > 0 else 0
    largest_frac = largest_zone_area / total_waste_area if total_waste_area > 0 else 0

    return {
        "total_zones": total_zones,
        "total_waste_mm2": total_waste_area,
        "largest_zone_mm2": largest_zone_area,
        "largest_zone_frac": round(largest_frac, 4),
        "compactness": round(compactness, 4),
        "per_sheet": per_sheet,
    }


def main():
    all_results = {}

    for seed in range(1, SEEDS + 1):
        print(f"\n=== seed {seed} ===", flush=True)
        seed_result = {}

        # Run A: group_shift DISABLED
        req_a = json.loads(json.dumps(base_req))
        req_a["params"]["seed"] = seed
        req_a["params"]["group_shift"] = {"enabled": False}

        t0 = time.time()
        resp_a = call_optimize(req_a)
        time_a = round(time.time() - t0, 1)

        if resp_a is None:
            print(f"  A: FAILED", flush=True)
            continue

        utils_a = sheet_utils(resp_a)
        lead_a = sum(sorted([u["util"] for u in utils_a], reverse=True)[:-1]) / max(1, len(utils_a) - 1) if len(utils_a) > 1 else utils_a[0]["util"]
        wr_a = waste_regions_from_response(resp_a)
        vis_a = compute_visual_waste_metrics(resp_a["solutions"])

        print(f"  A (no gs): {len(utils_a)}s lead={lead_a:.2f}% wr={wr_a} zones_no_kerf={vis_a['total_zones']} compact={vis_a['compactness']:.3f} largest_frac={vis_a['largest_zone_frac']:.3f}", flush=True)

        # Run B: group_shift ENABLED
        req_b = json.loads(json.dumps(base_req))
        req_b["params"]["seed"] = seed
        req_b["params"]["group_shift"] = {"enabled": True, "min_shift_mm": 5.0, "max_passes": 4}

        t0 = time.time()
        resp_b = call_optimize(req_b)
        time_b = round(time.time() - t0, 1)

        if resp_b is None:
            print(f"  B: FAILED", flush=True)
            continue

        utils_b = sheet_utils(resp_b)
        lead_b = sum(sorted([u["util"] for u in utils_b], reverse=True)[:-1]) / max(1, len(utils_b) - 1) if len(utils_b) > 1 else utils_b[0]["util"]
        wr_b = waste_regions_from_response(resp_b)
        vis_b = compute_visual_waste_metrics(resp_b["solutions"])

        gs = resp_b.get("summary", {}).get("group_shift") or {}

        print(f"  B (gs on): {len(utils_b)}s lead={lead_b:.2f}% wr={wr_b} zones_no_kerf={vis_b['total_zones']} compact={vis_b['compactness']:.3f} largest_frac={vis_b['largest_zone_frac']:.3f} moves={gs.get('moves_applied', 0)} closed={gs.get('corridor_closed_area_mm2', 0):.0f}", flush=True)

        # Save SVGs for visual comparison
        for sol_idx, sol in enumerate(resp_a["solutions"]):
            for ss in sol.get("used_stock", []):
                svg = ss.get("svg")
                if svg:
                    with open(os.path.join(OUT_DIR, f"s{seed}_before_sheet{sol_idx}.svg"), "w", encoding="utf-8") as f:
                        f.write(svg)

        for sol_idx, sol in enumerate(resp_b["solutions"]):
            for ss in sol.get("used_stock", []):
                svg = ss.get("svg")
                if svg:
                    with open(os.path.join(OUT_DIR, f"s{seed}_after_sheet{sol_idx}.svg"), "w", encoding="utf-8") as f:
                        f.write(svg)

        seed_result = {
            "seed": seed,
            "before": {
                "sheets": len(utils_a), "lead_util": round(lead_a, 2),
                "waste_regions_with_kerf": wr_a,
                "visual": vis_a,
                "time_s": time_a,
            },
            "after": {
                "sheets": len(utils_b), "lead_util": round(lead_b, 2),
                "waste_regions_with_kerf": wr_b,
                "visual": vis_b,
                "group_shift": gs,
                "time_s": time_b,
            },
        }

        # Compute deltas
        delta_zones_kerf = wr_b - wr_a
        delta_zones_no_kerf = vis_b["total_zones"] - vis_a["total_zones"]
        delta_compact = vis_b["compactness"] - vis_a["compactness"]
        delta_largest_frac = vis_b["largest_zone_frac"] - vis_a["largest_zone_frac"]

        seed_result["delta"] = {
            "zones_with_kerf": delta_zones_kerf,
            "zones_no_kerf": delta_zones_no_kerf,
            "compactness": round(delta_compact, 4),
            "largest_zone_frac": round(delta_largest_frac, 4),
            "corridor_closed_mm2": gs.get("corridor_closed_area_mm2", 0),
        }

        verdict = []
        if delta_zones_no_kerf < 0:
            verdict.append(f"zones↓{abs(delta_zones_no_kerf)}")
        elif delta_zones_no_kerf > 0:
            verdict.append(f"zones↑{delta_zones_no_kerf}")
        if delta_compact > 0.01:
            verdict.append(f"compact↑{delta_compact:.3f}")
        elif delta_compact < -0.01:
            verdict.append(f"compact↓{delta_compact:.3f}")
        if delta_largest_frac > 0.01:
            verdict.append(f"lfrac↑{delta_largest_frac:.3f}")

        print(f"  DELTA: zones_kerf={delta_zones_kerf} zones_no_kerf={delta_zones_no_kerf} compact={delta_compact:+.4f} lfrac={delta_largest_frac:+.4f} {' '.join(verdict) if verdict else 'neutral'}", flush=True)

        all_results[seed] = seed_result

    # Summary
    n = len(all_results)
    if n == 0:
        return

    zones_kerf_improved = sum(1 for r in all_results.values() if r["delta"]["zones_with_kerf"] < 0)
    zones_kerf_worsened = sum(1 for r in all_results.values() if r["delta"]["zones_with_kerf"] > 0)
    zones_nokerf_improved = sum(1 for r in all_results.values() if r["delta"]["zones_no_kerf"] < 0)
    zones_nokerf_worsened = sum(1 for r in all_results.values() if r["delta"]["zones_no_kerf"] > 0)
    compact_improved = sum(1 for r in all_results.values() if r["delta"]["compactness"] > 0.01)
    compact_worsened = sum(1 for r in all_results.values() if r["delta"]["compactness"] < -0.01)
    lfrac_improved = sum(1 for r in all_results.values() if r["delta"]["largest_zone_frac"] > 0.01)
    lfrac_worsened = sum(1 for r in all_results.values() if r["delta"]["largest_zone_frac"] < -0.01)

    avg_delta_zones_kerf = sum(r["delta"]["zones_with_kerf"] for r in all_results.values()) / n
    avg_delta_zones_nokerf = sum(r["delta"]["zones_no_kerf"] for r in all_results.values()) / n
    avg_delta_compact = sum(r["delta"]["compactness"] for r in all_results.values()) / n
    avg_delta_lfrac = sum(r["delta"]["largest_zone_frac"] for r in all_results.values()) / n
    avg_closed = sum(r["delta"]["corridor_closed_mm2"] for r in all_results.values()) / n

    print(f"\n\n=== SUMMARY ({n} seeds) ===", flush=True)
    print(f"zones (with kerf):    improved={zones_kerf_improved} worsened={zones_kerf_worsened} neutral={n - zones_kerf_improved - zones_kerf_worsened} avg_delta={avg_delta_zones_kerf:+.2f}", flush=True)
    print(f"zones (NO kerf):      improved={zones_nokerf_improved} worsened={zones_nokerf_worsened} neutral={n - zones_nokerf_improved - zones_nokerf_worsened} avg_delta={avg_delta_zones_nokerf:+.2f}", flush=True)
    print(f"compactness:          improved={compact_improved} worsened={compact_worsened} avg_delta={avg_delta_compact:+.4f}", flush=True)
    print(f"largest_zone_frac:    improved={lfrac_improved} worsened={lfrac_worsened} avg_delta={avg_delta_lfrac:+.4f}", flush=True)
    print(f"avg corridor closed:  {avg_closed:.0f} mm²", flush=True)

    print(f"\n=== PER-SEED TABLE ===", flush=True)
    print(f"| seed | zones_before | zones_after | delta_nokerf | compact_Δ | lfrac_Δ | closed_mm2 |", flush=True)
    print(f"|------|-------------|-------------|-------------|-----------|---------|------------|", flush=True)
    for seed in sorted(all_results.keys()):
        r = all_results[seed]
        print(f"| {seed} | {r['before']['visual']['total_zones']} | {r['after']['visual']['total_zones']} | {r['delta']['zones_no_kerf']:+d} | {r['delta']['compactness']:+.4f} | {r['delta']['largest_zone_frac']:+.4f} | {r['delta']['corridor_closed_mm2']:.0f} |", flush=True)

    with open(os.path.join(OUT_DIR, "v39_reaudit_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nResults + SVGs saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()