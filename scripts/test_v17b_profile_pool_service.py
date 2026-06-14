"""V17b hypothesis test: built-in service profile_pool.

Calls `/v1/optimize` once per seed with `params.profile_pool` enabled.  The
service runs the configured zone-penalty profiles internally and returns the
winner plus `summary.profile_pool` telemetry.
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
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v17b_profile_pool_service"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8088")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "8"))
SEED_LIST = [
    int(x.strip())
    for x in os.environ.get("FREECUT_SEED_LIST", "").split(",")
    if x.strip()
]
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
PROFILES = [
    float(x.strip())
    for x in os.environ.get("FREECUT_PROFILE_POOL", "0.2,0.3,0.4,0.5").split(",")
    if x.strip()
]
MAX_LEAD_DROP_PP = float(os.environ.get("FREECUT_PROFILE_POOL_MAX_LEAD_DROP_PP", "0.8"))
SEED_OFFSETS = [
    int(x.strip())
    for x in os.environ.get("FREECUT_PROFILE_POOL_SEED_OFFSETS", "").split(",")
    if x.strip()
]
RESCUE_ZONES_GT = os.environ.get("FREECUT_PROFILE_POOL_RESCUE_ZONES_GT")
RESCUE_CORNER_BELOW = os.environ.get("FREECUT_PROFILE_POOL_RESCUE_CORNER_BELOW_MM2")

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
base_req["params"]["profile_pool"] = {
    "enabled": True,
    "zone_penalties": PROFILES,
    "fill_penalty": 0.1,
    "max_lead_drop_pp": MAX_LEAD_DROP_PP,
}
if SEED_OFFSETS:
    base_req["params"]["profile_pool"]["seed_offsets"] = SEED_OFFSETS
if RESCUE_ZONES_GT not in (None, ""):
    base_req["params"]["profile_pool"]["rescue_when_zones_gt"] = int(RESCUE_ZONES_GT)
if RESCUE_CORNER_BELOW not in (None, ""):
    base_req["params"]["profile_pool"]["rescue_when_max_corner_below_mm2"] = float(
        RESCUE_CORNER_BELOW
    )


def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    request = urllib.request.Request(
        f"http://localhost:{PORT}/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read())


def sheet_utils(response):
    out = []
    for solution in response.get("solutions", []):
        trim = solution.get("trim_mm", {})
        width = solution["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        height = solution["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(
            p["width_mm"] * p["height_mm"]
            for p in solution.get("placements", [])
        )
        out.append(used / (width * height) * 100.0 if width > 0 and height > 0 else 0.0)
    return out


def row_from_response(seed, data):
    pool = data.get("summary", {}).get("profile_pool") or {}
    utils = sheet_utils(data)
    return {
        "seed": seed,
        "sheets": len(data.get("solutions", [])),
        "zone_penalty": pool.get("winner_zone_penalty"),
        "n_waste_regions": pool.get("winner_waste_regions", 0),
        "lead_util": round(pool.get("winner_lead_util_pct", 0.0), 2),
        "max_corner_mm2": round(pool.get("winner_max_corner_mm2", 0.0)),
        "winner_seed": pool.get("winner_seed"),
        "rescue_triggered": pool.get("rescue_triggered", False),
        "seed_offsets_used": pool.get("seed_offsets_used", []),
        "utils": [round(u, 1) for u in utils],
        "candidates_completed": pool.get("candidates_completed", 0),
        "candidates_timed_out": pool.get("candidates_timed_out", 0),
        "data": data,
    }


def main():
    rows = []
    started = time.time()
    print(f"profiles={PROFILES}, max_lead_drop_pp={MAX_LEAD_DROP_PP}", flush=True)
    if SEED_OFFSETS:
        print(
            f"seed_offsets={SEED_OFFSETS}, rescue_zones_gt={RESCUE_ZONES_GT}, "
            f"rescue_corner_below={RESCUE_CORNER_BELOW}",
            flush=True,
        )
    seeds_to_run = SEED_LIST or list(range(1, SEEDS + 1))
    for seed in seeds_to_run:
        try:
            data = call_optimize(seed)
        except Exception as exc:
            print(f"  seed={seed:2d}: ERROR {exc}", flush=True)
            continue
        row = row_from_response(seed, data)
        rows.append(row)
        print(
            f"  seed={seed:2d}: sheets={row['sheets']}, zp={row['zone_penalty']}, "
            f"winner_seed={row['winner_seed']}, rescue={row['rescue_triggered']}, "
            f"lead={row['lead_util']:5.2f}%, regions={row['n_waste_regions']}, "
            f"corner={row['max_corner_mm2'] / 1e3:.0f}k, "
            f"completed={row['candidates_completed']}, timeouts={row['candidates_timed_out']}",
            flush=True,
        )

    n = max(1, len(rows))
    n_4 = sum(1 for r in rows if r["sheets"] == 4)
    avg_lead = sum(r["lead_util"] for r in rows) / n
    avg_regions = sum(r["n_waste_regions"] for r in rows) / n
    avg_max_corner = sum(r["max_corner_mm2"] for r in rows) / n
    n_regions_le_4 = sum(1 for r in rows if r["n_waste_regions"] <= 4)
    n_regions_le_5 = sum(1 for r in rows if r["n_waste_regions"] <= 5)

    print(f"\nV17b Service Profile Pool Summary ({len(rows)} seeds): elapsed {time.time() - started:.0f}s", flush=True)
    print(f"  4-sheet rate:                 {n_4}/{len(rows)}", flush=True)
    print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%", flush=True)
    print(f"  Avg waste regions per layout: {avg_regions:.2f}", flush=True)
    print(f"  Avg max corner rect:          {avg_max_corner / 1e3:.0f}k mm2", flush=True)
    print(f"  Layouts with <=4 regions:     {n_regions_le_4}/{len(rows)}", flush=True)
    print(f"  Layouts with <=5 regions:     {n_regions_le_5}/{len(rows)}", flush=True)

    ranked = sorted(rows, key=lambda r: (r["sheets"] != 4, r["n_waste_regions"], -r["lead_util"]))
    for i, row in enumerate(ranked[:5], start=1):
        svg = row.get("data", {}).get("artifacts", {}).get("svg", "")
        stem = f"rank_{i:02d}_zones{row['n_waste_regions']}_seed_{row['seed']}_zp{row['zone_penalty']:.2f}"
        if svg:
            with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
                f.write(svg)
        with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
            json.dump({k: v for k, v in row.items() if k != "data"}, f, indent=2, ensure_ascii=False)
        print(
            f"  rank {i}: seed={row['seed']}, zp={row['zone_penalty']:.2f}, "
            f"zones={row['n_waste_regions']}, lead={row['lead_util']}%, utils={row['utils']}",
            flush=True,
        )

    with open(os.path.join(OUT_DIR, "v17b_profile_pool_service_summary.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "profiles": PROFILES,
                "max_lead_drop_pp": MAX_LEAD_DROP_PP,
                "seed_offsets": SEED_OFFSETS,
                "rescue_when_zones_gt": (
                    int(RESCUE_ZONES_GT)
                    if RESCUE_ZONES_GT not in (None, "")
                    else None
                ),
                "rescue_when_max_corner_below_mm2": (
                    float(RESCUE_CORNER_BELOW)
                    if RESCUE_CORNER_BELOW not in (None, "")
                    else None
                ),
                "seeds": seeds_to_run,
                "results": [{k: v for k, v in r.items() if k != "data"} for r in rows],
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
