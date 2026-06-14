"""V29 hypothesis test: opt-in group-shift postprocess.

Runs the V27 balanced-quality profile-pool preset and enables the V29
postprocess that shifts peripheral side groups toward the denser anchor group.
The main metric is whether the postprocess closes internal corridor area
without changing sheet count or profile-pool quality metrics.
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
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v29_group_shift_postprocess"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8095")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "30"))
SEED_LIST = [
    int(x.strip())
    for x in os.environ.get("FREECUT_SEED_LIST", "").split(",")
    if x.strip()
]
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
PROFILE_POOL_PRESET = os.environ.get("FREECUT_PROFILE_POOL_PRESET", "balanced_quality")
GROUP_SHIFT_MIN_SHIFT_MM = float(os.environ.get("FREECUT_GROUP_SHIFT_MIN_SHIFT_MM", "5.0"))
GROUP_SHIFT_MAX_PASSES = int(os.environ.get("FREECUT_GROUP_SHIFT_MAX_PASSES", "4"))

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
    "preset": PROFILE_POOL_PRESET,
    "fill_penalty": 0.1,
    "max_lead_drop_pp": 0.8,
}
base_req["params"]["group_shift"] = {
    "enabled": True,
    "min_shift_mm": GROUP_SHIFT_MIN_SHIFT_MM,
    "max_passes": GROUP_SHIFT_MAX_PASSES,
}


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
            p["width_mm"] * p["height_mm"] for p in solution.get("placements", [])
        )
        out.append(used / (width * height) * 100.0 if width > 0 and height > 0 else 0.0)
    return out


def row_from_response(seed, data):
    summary = data.get("summary", {})
    pool = summary.get("profile_pool") or {}
    group_shift = summary.get("group_shift") or {}
    utils = sheet_utils(data)
    return {
        "seed": seed,
        "sheets": len(data.get("solutions", [])),
        "preset": pool.get("preset"),
        "zone_penalty": pool.get("winner_zone_penalty"),
        "n_waste_regions": pool.get("winner_waste_regions", 0),
        "lead_util": round(pool.get("winner_lead_util_pct", 0.0), 2),
        "max_corner_mm2": round(pool.get("winner_max_corner_mm2", 0.0)),
        "winner_seed": pool.get("winner_seed"),
        "rescue_triggered": pool.get("rescue_triggered", False),
        "rescue_zone_penalties_used": pool.get("rescue_zone_penalties_used", []),
        "rescue_candidates_rejected_by_guard": pool.get(
            "rescue_candidates_rejected_by_guard", 0
        ),
        "utils": [round(u, 1) for u in utils],
        "candidates_completed": pool.get("candidates_completed", 0),
        "candidates_timed_out": pool.get("candidates_timed_out", 0),
        "group_moves": group_shift.get("moves_applied", 0),
        "group_parts_moved": group_shift.get("parts_moved", 0),
        "group_passes_run": group_shift.get("passes_run", 0),
        "group_time_ms": group_shift.get("time_ms", 0),
        "group_closed_area_mm2": round(group_shift.get("corridor_closed_area_mm2", 0.0)),
        "group_opportunity_before_mm2": round(
            group_shift.get("corridor_opportunity_before_mm2", 0.0)
        ),
        "group_opportunity_after_mm2": round(
            group_shift.get("corridor_opportunity_after_mm2", 0.0)
        ),
        "group_opportunity_delta_mm2": round(
            group_shift.get("corridor_opportunity_delta_mm2", 0.0)
        ),
        "group_max_shift_mm": round(group_shift.get("max_shift_mm", 0.0), 3),
        "data": data,
    }


def write_ranked_artifacts(rows):
    ranked_quality = sorted(
        rows,
        key=lambda r: (
            r["sheets"] != 4,
            r["n_waste_regions"],
            -r["lead_util"],
            -r["group_closed_area_mm2"],
        ),
    )
    ranked_group = sorted(
        [r for r in rows if r["group_moves"] > 0],
        key=lambda r: (
            -r["group_closed_area_mm2"],
            -r["group_max_shift_mm"],
            r["n_waste_regions"],
        ),
    )

    saved = set()
    for label, ranked in (("quality", ranked_quality[:5]), ("group", ranked_group[:10])):
        for rank, row in enumerate(ranked, start=1):
            svg = row.get("data", {}).get("artifacts", {}).get("svg", "")
            stem = (
                f"{label}_{rank:02d}_zones{row['n_waste_regions']}_seed_{row['seed']}"
                f"_moves{row['group_moves']}_closed{row['group_closed_area_mm2']}"
            )
            if stem in saved:
                continue
            saved.add(stem)
            if svg:
                with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
                    f.write(svg)
            with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
                json.dump(
                    {k: v for k, v in row.items() if k != "data"},
                    f,
                    indent=2,
                    ensure_ascii=False,
                )

    return ranked_quality, ranked_group


def main():
    rows = []
    started = time.time()
    seeds_to_run = SEED_LIST or list(range(1, SEEDS + 1))
    print(
        f"preset={PROFILE_POOL_PRESET}, group_shift_min={GROUP_SHIFT_MIN_SHIFT_MM}, "
        f"group_shift_passes={GROUP_SHIFT_MAX_PASSES}, seeds={seeds_to_run}",
        flush=True,
    )
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
            f"lead={row['lead_util']:5.2f}%, zones={row['n_waste_regions']}, "
            f"corner={row['max_corner_mm2'] / 1e3:.0f}k, "
            f"group_moves={row['group_moves']}, parts={row['group_parts_moved']}, "
            f"closed={row['group_closed_area_mm2'] / 1e3:.0f}k, "
            f"max_shift={row['group_max_shift_mm']:.1f}mm, "
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
    moved_layouts = sum(1 for r in rows if r["group_moves"] > 0)
    total_moves = sum(r["group_moves"] for r in rows)
    total_parts = sum(r["group_parts_moved"] for r in rows)
    total_group_time_ms = sum(r["group_time_ms"] for r in rows)
    total_closed = sum(r["group_closed_area_mm2"] for r in rows)
    total_opportunity_before = sum(r["group_opportunity_before_mm2"] for r in rows)
    total_opportunity_after = sum(r["group_opportunity_after_mm2"] for r in rows)
    total_opportunity_delta = sum(r["group_opportunity_delta_mm2"] for r in rows)
    max_shift = max((r["group_max_shift_mm"] for r in rows), default=0.0)

    ranked_quality, ranked_group = write_ranked_artifacts(rows)

    print(
        f"\nV29 Group Shift Summary ({len(rows)} seeds): elapsed {time.time() - started:.0f}s",
        flush=True,
    )
    print(f"  4-sheet rate:                 {n_4}/{len(rows)}", flush=True)
    print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%", flush=True)
    print(f"  Avg waste regions per layout: {avg_regions:.2f}", flush=True)
    print(f"  Avg max corner rect:          {avg_max_corner / 1e3:.0f}k mm2", flush=True)
    print(f"  Layouts with <=4 regions:     {n_regions_le_4}/{len(rows)}", flush=True)
    print(f"  Layouts with <=5 regions:     {n_regions_le_5}/{len(rows)}", flush=True)
    print(f"  Layouts moved by group_shift: {moved_layouts}/{len(rows)}", flush=True)
    print(f"  Total group moves/parts:      {total_moves}/{total_parts}", flush=True)
    print(f"  Total group_shift time:       {total_group_time_ms}ms", flush=True)
    print(f"  Total corridor closed area:   {total_closed / 1e3:.0f}k mm2", flush=True)
    print(
        f"  Opportunity before/after:     {total_opportunity_before / 1e3:.0f}k -> "
        f"{total_opportunity_after / 1e3:.0f}k mm2 (delta {total_opportunity_delta / 1e3:.0f}k)",
        flush=True,
    )
    print(f"  Max accepted shift:           {max_shift:.1f}mm", flush=True)
    for i, row in enumerate(ranked_quality[:5], start=1):
        print(
            f"  quality rank {i}: seed={row['seed']}, zones={row['n_waste_regions']}, "
            f"lead={row['lead_util']}%, moves={row['group_moves']}, "
            f"closed={row['group_closed_area_mm2'] / 1e3:.0f}k",
            flush=True,
        )
    for i, row in enumerate(ranked_group[:5], start=1):
        print(
            f"  group rank {i}: seed={row['seed']}, zones={row['n_waste_regions']}, "
            f"moves={row['group_moves']}, parts={row['group_parts_moved']}, "
            f"closed={row['group_closed_area_mm2'] / 1e3:.0f}k",
            flush=True,
        )

    with open(os.path.join(OUT_DIR, "v29_group_shift_summary.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "profile_pool_preset": PROFILE_POOL_PRESET,
                "group_shift_min_shift_mm": GROUP_SHIFT_MIN_SHIFT_MM,
                "group_shift_max_passes": GROUP_SHIFT_MAX_PASSES,
                "seeds": seeds_to_run,
                "summary": {
                    "four_sheet_rate": [n_4, len(rows)],
                    "avg_lead_util_pct": avg_lead,
                    "avg_waste_regions": avg_regions,
                    "avg_max_corner_mm2": avg_max_corner,
                    "regions_le_4": [n_regions_le_4, len(rows)],
                    "regions_le_5": [n_regions_le_5, len(rows)],
                    "moved_layouts": [moved_layouts, len(rows)],
                    "total_group_moves": total_moves,
                    "total_group_parts_moved": total_parts,
                    "total_group_time_ms": total_group_time_ms,
                    "total_closed_area_mm2": total_closed,
                    "total_opportunity_before_mm2": total_opportunity_before,
                    "total_opportunity_after_mm2": total_opportunity_after,
                    "total_opportunity_delta_mm2": total_opportunity_delta,
                    "max_shift_mm": max_shift,
                },
                "results": [{k: v for k, v in r.items() if k != "data"} for r in rows],
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
