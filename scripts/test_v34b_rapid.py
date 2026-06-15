"""V34b rapid test: monotonicity_penalty with single-request GA override (no profile_pool).

This script sends simple requests with ga_override.monotonicity_penalty
directly, avoiding the expensive profile_pool/portfolio/partition machinery.

Environment variables:
  FREECUT_PORT  - server port (default 8092)
  FREECUT_SEEDS - number of seeds 1..N (default 10)
  FREECUT_OUT_DIR - output directory
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
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v34b_rapid"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "10"))

MP_VALUES = [0.0, 0.1, 0.2, 0.3, 0.5, 0.8]
ZONE_PENALTIES = [0.2, 0.3, 0.4, 0.5]

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 3000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = False
base_req["params"]["portfolio"] = None
base_req["params"]["retry_strategy"] = None
base_req["params"]["max_retry_attempts"] = None
base_req["params"]["partition"] = None

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
        with urllib.request.urlopen(r, timeout=60) as resp:
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
    total = 0
    for sol in response.get("solutions", []):
        for ss in sol.get("used_stock", []):
            total += ss.get("waste_regions", 0)
    return total


def main():
    all_configs = {}

    for mp in MP_VALUES:
        tag = f"mp{mp:.1f}".replace(".", "_")
        print(f"\n=== monotonicity_penalty={mp} ({tag}) ===", flush=True)

        for zp in ZONE_PENALTIES:
            ztag = f"zp{zp:.1f}".replace(".", "_")
            config_tag = f"{tag}_{ztag}"
            print(f"\n  --- zone_penalty={zp} ---", flush=True)
            config_results = []

            for seed in range(1, SEEDS + 1):
                req = json.loads(json.dumps(base_req))
                req["params"]["seed"] = seed
                req["params"]["ga_override"] = {
                    "zone_penalty": zp,
                    "fill_penalty": 0.1,
                    "monotonicity_penalty": mp,
                }

                t0 = time.time()
                resp = call_optimize(req)
                elapsed = time.time() - t0

                if resp is None:
                    print(f"    seed {seed}: FAILED", flush=True)
                    continue

                solutions = resp.get("solutions", [])
                n_sheets = len(solutions)
                utils = sheet_utils(resp)
                min_util = min(utils) if utils else 0
                lead_util = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else (utils[0] if utils else 0)
                n_waste_regions = response_waste_regions(resp)

                row = {
                    "seed": seed,
                    "sheets": n_sheets,
                    "min_util": round(min_util, 2),
                    "lead_util": round(lead_util, 2),
                    "n_waste_regions": n_waste_regions,
                    "elapsed_s": round(elapsed, 1),
                }
                config_results.append(row)
                print(f"    seed {seed}: {n_sheets}s lead={lead_util:.2f}% zones={n_waste_regions}", flush=True)

            all_configs[config_tag] = config_results

            if config_results:
                n4 = sum(1 for r in config_results if r["sheets"] == 4)
                avg_lead = sum(r["lead_util"] for r in config_results) / len(config_results)
                avg_zones = sum(r["n_waste_regions"] for r in config_results) / len(config_results)
                le4 = sum(1 for r in config_results if r["n_waste_regions"] <= 4)
                le5 = sum(1 for r in config_results if r["n_waste_regions"] <= 5)
                print(f"\n  SUMMARY mp={mp} zp={zp}: 4s={n4}/{len(config_results)} avg_lead={avg_lead:.2f}% avg_zones={avg_zones:.1f} le4={le4} le5={le5}", flush=True)

    # Save results
    with open(os.path.join(OUT_DIR, "v34b_rapid_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_configs, f, indent=2, ensure_ascii=False)

    # Print comparison table per mp
    print("\n\n=== COMPARISON TABLE ===", flush=True)
    print("| mp-zp | 4s | lead_util | avg zones | <=4 | <=5 |", flush=True)
    print("|-------|-----|-----------|-----------|-----|-----|", flush=True)
    for config_tag, results in all_configs.items():
        if not results:
            continue
        n = len(results)
        n4 = sum(1 for r in results if r["sheets"] == 4)
        avg_lead = sum(r["lead_util"] for r in results) / n
        avg_zones = sum(r["n_waste_regions"] for r in results) / n
        le4 = sum(1 for r in results if r["n_waste_regions"] <= 4)
        le5 = sum(1 for r in results if r["n_waste_regions"] <= 5)
        print(f"| {config_tag} | {n4}/{n} | {avg_lead:.2f}% | {avg_zones:.1f} | {le4} | {le5} |", flush=True)

    # Aggregated per mp (across all zp)
    print("\n=== AGGREGATED PER monotonicity_penalty ===", flush=True)
    print("| mp | avg_lead | avg_zones | le4 | le5 |", flush=True)
    print("|----|----------|-----------|-----|-----|", flush=True)
    for mp in MP_VALUES:
        tag_prefix = f"mp{mp:.1f}".replace(".", "_")
        all_rows = []
        for config_tag, results in all_configs.items():
            if config_tag.startswith(tag_prefix):
                all_rows.extend(results)
        if all_rows:
            n = len(all_rows)
            avg_lead = sum(r["lead_util"] for r in all_rows) / n
            avg_zones = sum(r["n_waste_regions"] for r in all_rows) / n
            le4 = sum(1 for r in all_rows if r["n_waste_regions"] <= 4)
            le5 = sum(1 for r in all_rows if r["n_waste_regions"] <= 5)
            print(f"| {mp:.1f} | {avg_lead:.2f}% | {avg_zones:.1f} | {le4}/{n} | {le5}/{n} |", flush=True)

    print(f"\nResults saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()