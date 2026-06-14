"""V19 research: candidate grid for stubborn profile_pool outliers.

Runs single-profile service-side `profile_pool` candidates across seed offsets
so the measured zones/corner metrics come from the same Rust telemetry used by
V17b/V18 selection.
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
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v19_outlier_candidate_grid"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8088")
SEEDS = [
    int(x.strip())
    for x in os.environ.get("FREECUT_SEED_LIST", "19,22,26").split(",")
    if x.strip()
]
PROFILES = [
    float(x.strip())
    for x in os.environ.get("FREECUT_PROFILE_POOL", "0.2,0.3,0.5").split(",")
    if x.strip()
]
SEED_OFFSETS = [
    int(x.strip())
    for x in os.environ.get("FREECUT_SEED_OFFSETS", "0,1000003,2000006").split(",")
    if x.strip()
]
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
MAX_LEAD_DROP_PP = float(os.environ.get("FREECUT_PROFILE_POOL_MAX_LEAD_DROP_PP", "0.4"))

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


def call_candidate(base_seed, offset, profile):
    req = json.loads(json.dumps(base_req))
    actual_seed = base_seed + offset
    req["params"]["seed"] = actual_seed
    req["params"]["profile_pool"] = {
        "enabled": True,
        "zone_penalties": [profile],
        "fill_penalty": 0.1,
        "max_lead_drop_pp": MAX_LEAD_DROP_PP,
    }
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


def row_from_response(base_seed, offset, profile, data):
    pool = data.get("summary", {}).get("profile_pool") or {}
    return {
        "base_seed": base_seed,
        "offset": offset,
        "seed": base_seed + offset,
        "zone_penalty": profile,
        "sheets": len(data.get("solutions", [])),
        "n_waste_regions": pool.get("winner_waste_regions", 0),
        "lead_util": round(pool.get("winner_lead_util_pct", 0.0), 2),
        "max_corner_mm2": round(pool.get("winner_max_corner_mm2", 0.0)),
        "utils": [round(u, 1) for u in sheet_utils(data)],
        "time_ms": data.get("summary", {}).get("time_ms", 0),
        "data": data,
    }


def candidate_key(row):
    return (
        row["sheets"] != 4,
        row["n_waste_regions"],
        -row["lead_util"],
        -row["max_corner_mm2"],
    )


def main():
    started = time.time()
    rows = []
    winners = []
    print(
        f"seeds={SEEDS}, profiles={PROFILES}, offsets={SEED_OFFSETS}",
        flush=True,
    )
    for base_seed in SEEDS:
        seed_rows = []
        for offset in SEED_OFFSETS:
            for profile in PROFILES:
                try:
                    data = call_candidate(base_seed, offset, profile)
                    row = row_from_response(base_seed, offset, profile, data)
                except Exception as exc:
                    print(
                        f"  seed={base_seed:2d} offset={offset} zp={profile:.2f}: ERROR {exc}",
                        flush=True,
                    )
                    continue
                rows.append(row)
                seed_rows.append(row)
                print(
                    f"  seed={base_seed:2d} offset={offset:7d} zp={profile:.2f}: "
                    f"sheets={row['sheets']}, zones={row['n_waste_regions']}, "
                    f"lead={row['lead_util']:5.2f}%, corner={row['max_corner_mm2']/1e3:.0f}k",
                    flush=True,
                )
        if seed_rows:
            winner = min(seed_rows, key=candidate_key)
            winners.append(winner)
            print(
                f"    -> winner seed={winner['seed']} offset={winner['offset']} "
                f"zp={winner['zone_penalty']:.2f}, zones={winner['n_waste_regions']}, "
                f"lead={winner['lead_util']:.2f}%, corner={winner['max_corner_mm2']/1e3:.0f}k",
                flush=True,
            )

    elapsed = time.time() - started
    n = max(1, len(winners))
    n_4 = sum(1 for r in winners if r["sheets"] == 4)
    avg_zones = sum(r["n_waste_regions"] for r in winners) / n
    avg_lead = sum(r["lead_util"] for r in winners) / n
    print(f"\nV19 Outlier Candidate Grid Summary ({len(winners)} seeds): elapsed {elapsed:.0f}s")
    print(f"  candidates evaluated:         {len(rows)}")
    print(f"  4-sheet winners:              {n_4}/{len(winners)}")
    print(f"  Avg winner zones:             {avg_zones:.2f}")
    print(f"  Avg winner lead:              {avg_lead:.2f}%")
    print(f"  Winners <=5 zones:            {sum(r['n_waste_regions'] <= 5 for r in winners)}/{len(winners)}")

    ranked = sorted(winners, key=candidate_key)
    for i, row in enumerate(ranked, start=1):
        svg = row.get("data", {}).get("artifacts", {}).get("svg", "")
        stem = (
            f"rank_{i:02d}_base{row['base_seed']}_seed{row['seed']}"
            f"_zones{row['n_waste_regions']}_zp{row['zone_penalty']:.2f}"
        )
        if svg:
            with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
                f.write(svg)
        with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
            json.dump({k: v for k, v in row.items() if k != "data"}, f, indent=2, ensure_ascii=False)

    with open(os.path.join(OUT_DIR, "v19_outlier_candidate_grid_summary.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "seeds": SEEDS,
                "profiles": PROFILES,
                "seed_offsets": SEED_OFFSETS,
                "max_lead_drop_pp": MAX_LEAD_DROP_PP,
                "winners": [{k: v for k, v in r.items() if k != "data"} for r in winners],
                "rows": [{k: v for k, v in r.items() if k != "data"} for r in rows],
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
