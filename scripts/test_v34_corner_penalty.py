"""V34 hypothesis test: corner concentration penalty in GA fitness.

Calls `/v1/optimize` with profile_pool + varying corner_penalty values.
Compares cp=0.0 (baseline) vs cp=0.3/0.5/0.8 with the same zone_penalties.

Environment variables:
  FREECUT_PORT           - server port (default 8092)
  FREECUT_SEEDS          - number of seeds 1..N (default 8)
  FREECUT_TIME_LIMIT_MS  - per-request time budget (default 10000)
  FREECUT_SHEET_BUDGET_MS - partition budget (default 20000)
  FREECUT_OUT_DIR        - output directory
  FREECUT_CORNER_PENALTIES - comma-separated cp values (default "0.0,0.3,0.5,0.8")
"""

import json
import os
import sys
import time
import urllib.request

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.environ.get(
    "FREECUT_OUT_DIR",
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v34_corner_penalty"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "8"))
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
CORNER_PENALTIES = [
    float(x.strip())
    for x in os.environ.get("FREECUT_CORNER_PENALTIES", "0.0,0.3,0.5,0.8").split(",")
    if x.strip()
]
ZONE_PENALTIES = [
    float(x.strip())
    for x in os.environ.get("FREECUT_ZONE_PENALTIES", "0.2,0.3,0.4,0.5").split(",")
    if x.strip()
]

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

BASE_URL = f"http://127.0.0.1:{PORT}"


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
        body = e.read().decode("utf-8", errors="replace")
        print(f"  HTTP {e.code}: {body[:200]}", flush=True)
        return None


def sheet_utils(response):
    out = []
    for solution in response.get("solutions", []):
        trim = solution.get("trim_mm", {})
        w = solution["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = solution["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(p["width_mm"] * p["height_mm"] for p in solution.get("placements", []))
        out.append(used / (w * h) * 100.0 if w > 0 and h > 0 else 0.0)
    return out


def response_waste_regions(response):
    pool = response.get("summary", {}).get("profile_pool") or {}
    wr = pool.get("winner_waste_regions", 0)
    if wr > 0:
        return wr
    total = 0
    for sol in response.get("solutions", []):
        for ss in sol.get("used_stock", []):
            total += ss.get("waste_regions", 0)
    return total


def main():
    all_configs = {}

    for cp in CORNER_PENALTIES:
        tag = f"cp{cp:.1f}".replace(".", "_")
        print(f"\n=== corner_penalty={cp} ({tag}) ===", flush=True)
        cp_results = []

        for seed in range(1, SEEDS + 1):
            req = json.loads(json.dumps(base_req))
            req["params"]["seed"] = seed
            req["params"]["profile_pool"] = {
                "enabled": True,
                "zone_penalties": ZONE_PENALTIES,
                "fill_penalty": 0.1,
                "max_lead_drop_pp": 0.8,
                "corner_penalty": cp,
            }

            t0 = time.time()
            resp = call_optimize(req)
            elapsed = time.time() - t0

            if resp is None:
                print(f"  seed {seed}: FAILED", flush=True)
                continue

            summary = resp.get("summary", {})
            solutions = resp.get("solutions", [])
            n_sheets = len(solutions)
            utils = sheet_utils(resp)
            min_util = min(utils) if utils else 0
            lead_util = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else (utils[0] if utils else 0)
            n_waste_regions = response_waste_regions(resp)
            max_corner = summary.get("max_corner_area_mm2", 0)
            pool = summary.get("profile_pool") or {}

            row = {
                "seed": seed,
                "sheets": n_sheets,
                "min_util": round(min_util, 2),
                "lead_util": round(lead_util, 2),
                "utils": [round(u, 2) for u in utils],
                "n_waste_regions": n_waste_regions,
                "max_corner_mm2": max_corner,
                "elapsed_s": round(elapsed, 1),
                "partition_applied": summary.get("partition", {}).get("applied", False),
                "densest_zones": summary.get("partition", {}).get("densest_zones", []),
                "winner_zone_penalty": pool.get("winner_zone_penalty"),
                "peel_zone_penalties_used": summary.get("partition", {}).get("peel_zone_penalties_used", []),
            }
            cp_results.append(row)
            print(f"  seed {seed}: {n_sheets}s lead={lead_util:.2f}% zones={n_waste_regions} corner={max_corner:.0f}", flush=True)

            # Save best SVG for this cp/seed combo if it has few zones
            if n_waste_regions <= 4 and n_sheets == 4:
                svg_idx = 0
                for sol in solutions:
                    for ss in sol.get("used_stock", []):
                        svg = ss.get("svg")
                        if svg:
                            fname = f"{tag}_s{n_sheets}_z{n_waste_regions}_seed{seed}_sheet{svg_idx}.svg"
                            with open(os.path.join(OUT_DIR, fname), "w", encoding="utf-8") as f:
                                f.write(svg)
                            svg_idx += 1

        all_configs[cp] = cp_results

        # Summary for this cp
        if cp_results:
            n4 = sum(1 for r in cp_results if r["sheets"] == 4)
            avg_lead = sum(r["lead_util"] for r in cp_results) / len(cp_results)
            avg_zones = sum(r["n_waste_regions"] for r in cp_results) / len(cp_results)
            avg_corner = sum(r["max_corner_mm2"] for r in cp_results) / len(cp_results)
            le4 = sum(1 for r in cp_results if r["n_waste_regions"] <= 4)
            le5 = sum(1 for r in cp_results if r["n_waste_regions"] <= 5)
            print(f"\n  SUMMARY cp={cp}: 4s={n4}/{len(cp_results)} avg_lead={avg_lead:.2f}% avg_zones={avg_zones:.1f} le4={le4} le5={le5} avg_corner={avg_corner:.0f}", flush=True)

    # Save full results
    summary = {"configs": {}}
    for cp, results in all_configs.items():
        tag = f"cp{cp:.1f}".replace(".", "_")
        n = len(results)
        if n == 0:
            continue
        summary["configs"][tag] = {
            "corner_penalty": cp,
            "n_seeds": n,
            "n_4_sheet": sum(1 for r in results if r["sheets"] == 4),
            "avg_lead_util": round(sum(r["lead_util"] for r in results) / n, 2),
            "avg_min_util": round(sum(r["min_util"] for r in results) / n, 2),
            "avg_regions": round(sum(r["n_waste_regions"] for r in results) / n, 2),
            "n_regions_le_4": sum(1 for r in results if r["n_waste_regions"] <= 4),
            "n_regions_le_5": sum(1 for r in results if r["n_waste_regions"] <= 5),
            "avg_max_corner_k": round(sum(r["max_corner_mm2"] for r in results) / n / 1000, 0),
            "results": results,
        }

    with open(os.path.join(OUT_DIR, "v34_corner_penalty_summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)

    print(f"\nResults saved to {OUT_DIR}", flush=True)

    # Print comparison table
    print("\n=== COMPARISON TABLE ===", flush=True)
    print(f"| cp | 4-sheet | lead_util | min_util | avg zones | <=4 | <=5 | avg_corner |", flush=True)
    print(f"|----|---------|-----------|----------|-----------|-----|-----|------------|", flush=True)
    for cp in CORNER_PENALTIES:
        tag = f"cp{cp:.1f}".replace(".", "_")
        if tag in summary["configs"]:
            c = summary["configs"][tag]
            print(f"| {cp:.1f} | {c['n_4_sheet']}/{c['n_seeds']} | {c['avg_lead_util']:.2f}% | {c['avg_min_util']:.2f}% | {c['avg_regions']:.1f} | {c['n_regions_le_4']} | {c['n_regions_le_5']} | {c['avg_max_corner_k']:.0f}k |", flush=True)


if __name__ == "__main__":
    main()