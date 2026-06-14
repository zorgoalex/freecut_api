"""V31 paired group-shift benchmark.

Calls the service once per seed with `group_shift.debug_artifacts=true`.
The response contains the layout immediately before group-shift, the final
layout after group-shift, and a visual SVG diff. This avoids the V29/V30
problem where separate off/on HTTP calls could produce different optimizer
placements because of time cutoffs.
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
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v31_paired_group_shift_diff"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8096")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "12"))
SEED_LIST = [
    int(x.strip())
    for x in os.environ.get("FREECUT_SEED_LIST", "").split(",")
    if x.strip()
]
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "3000"))
RESTARTS = int(os.environ.get("FREECUT_RESTARTS", "3"))
GROUP_SHIFT_MIN_SHIFT_MM = float(os.environ.get("FREECUT_GROUP_SHIFT_MIN_SHIFT_MM", "5.0"))
GROUP_SHIFT_MAX_PASSES = int(os.environ.get("FREECUT_GROUP_SHIFT_MAX_PASSES", "4"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = TIME_LIMIT_MS
base_req["params"]["restarts"] = RESTARTS
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
base_req["params"]["retry_strategy"] = "disabled"
for optional in ("portfolio", "beam", "alns", "profile_pool", "partition"):
    base_req["params"].pop(optional, None)
base_req["params"]["group_shift"] = {
    "enabled": True,
    "debug_artifacts": True,
    "min_shift_mm": GROUP_SHIFT_MIN_SHIFT_MM,
    "max_passes": GROUP_SHIFT_MAX_PASSES,
}


def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=300) as response:
        data = json.loads(response.read())
    return (time.perf_counter() - started) * 1000.0, data


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


def row_from_response(seed, wall_ms, data):
    summary = data.get("summary", {})
    group_shift = summary.get("group_shift") or {}
    return {
        "seed": seed,
        "wall_ms": round(wall_ms, 2),
        "service_time_ms": summary.get("time_ms"),
        "sheets": len(data.get("solutions", [])),
        "placements": sum(len(s.get("placements", [])) for s in data.get("solutions", [])),
        "utils": [round(u, 1) for u in sheet_utils(data)],
        "group_time_ms": group_shift.get("time_ms", 0),
        "group_moves": group_shift.get("moves_applied", 0),
        "group_parts_moved": group_shift.get("parts_moved", 0),
        "group_passes_run": group_shift.get("passes_run", 0),
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
    }


def save_seed_artifacts(seed, data, row):
    artifacts = data.get("artifacts") or {}
    prefix = (
        f"seed_{seed:02d}_moves{row['group_moves']}"
        f"_closed{row['group_closed_area_mm2']}"
    )
    files = {
        "before": artifacts.get("group_shift_before_svg"),
        "after": artifacts.get("svg"),
        "diff": artifacts.get("group_shift_diff_svg"),
    }
    for label, svg in files.items():
        if not svg:
            continue
        with open(os.path.join(OUT_DIR, f"{prefix}_{label}.svg"), "w", encoding="utf-8") as f:
            f.write(svg)
    compact_response = json.loads(json.dumps(data))
    if "artifacts" in compact_response:
        compact_response["artifacts"] = {
            key: f"<omitted {len(value)} chars>"
            for key, value in compact_response["artifacts"].items()
            if isinstance(value, str)
        }
    with open(os.path.join(OUT_DIR, f"{prefix}.json"), "w", encoding="utf-8") as f:
        json.dump({"row": row, "response": compact_response}, f, indent=2, ensure_ascii=False)


def main():
    rows = []
    started = time.time()
    seeds_to_run = SEED_LIST or list(range(1, SEEDS + 1))
    print(
        f"time_limit={TIME_LIMIT_MS}ms, restarts={RESTARTS}, "
        f"group_shift_min={GROUP_SHIFT_MIN_SHIFT_MM}, passes={GROUP_SHIFT_MAX_PASSES}, "
        f"seeds={seeds_to_run}",
        flush=True,
    )
    for seed in seeds_to_run:
        try:
            wall_ms, data = call_optimize(seed)
        except Exception as exc:
            print(f"  seed={seed:2d}: ERROR {exc}", flush=True)
            continue
        row = row_from_response(seed, wall_ms, data)
        rows.append(row)
        save_seed_artifacts(seed, data, row)
        print(
            f"  seed={seed:2d}: sheets={row['sheets']}, wall={row['wall_ms']:.1f}ms, "
            f"group_time={row['group_time_ms']}ms, moves={row['group_moves']}, "
            f"parts={row['group_parts_moved']}, closed={row['group_closed_area_mm2'] / 1e3:.0f}k, "
            f"opp={row['group_opportunity_before_mm2'] / 1e3:.0f}k->"
            f"{row['group_opportunity_after_mm2'] / 1e3:.0f}k, "
            f"max_shift={row['group_max_shift_mm']:.1f}mm",
            flush=True,
        )

    n = max(1, len(rows))
    moved_layouts = sum(1 for r in rows if r["group_moves"] > 0)
    total_moves = sum(r["group_moves"] for r in rows)
    total_parts = sum(r["group_parts_moved"] for r in rows)
    total_time = sum(r["group_time_ms"] for r in rows)
    total_closed = sum(r["group_closed_area_mm2"] for r in rows)
    total_before = sum(r["group_opportunity_before_mm2"] for r in rows)
    total_after = sum(r["group_opportunity_after_mm2"] for r in rows)
    total_delta = sum(r["group_opportunity_delta_mm2"] for r in rows)
    max_shift = max((r["group_max_shift_mm"] for r in rows), default=0.0)
    ranked = sorted(rows, key=lambda r: (-r["group_closed_area_mm2"], r["seed"]))

    summary = {
        "fixture": "multisheet_varied_4sheets",
        "time_limit_ms": TIME_LIMIT_MS,
        "restarts": RESTARTS,
        "group_shift_min_shift_mm": GROUP_SHIFT_MIN_SHIFT_MM,
        "group_shift_max_passes": GROUP_SHIFT_MAX_PASSES,
        "seeds": seeds_to_run,
        "elapsed_s": round(time.time() - started, 1),
        "summary": {
            "moved_layouts": [moved_layouts, len(rows)],
            "total_group_moves": total_moves,
            "total_group_parts_moved": total_parts,
            "total_group_time_ms": total_time,
            "total_closed_area_mm2": total_closed,
            "total_opportunity_before_mm2": total_before,
            "total_opportunity_after_mm2": total_after,
            "total_opportunity_delta_mm2": total_delta,
            "avg_closed_area_mm2": total_closed / n,
            "avg_opportunity_delta_mm2": total_delta / n,
            "max_shift_mm": max_shift,
        },
        "results": rows,
    }
    with open(os.path.join(OUT_DIR, "v31_paired_group_shift_summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)

    print(f"\nV31 Paired Summary ({len(rows)} seeds): elapsed {summary['elapsed_s']:.1f}s")
    print(f"  Layouts moved:          {moved_layouts}/{len(rows)}")
    print(f"  Total moves/parts:      {total_moves}/{total_parts}")
    print(f"  Total group time:       {total_time}ms")
    print(f"  Total closed area:      {total_closed / 1e3:.0f}k mm2")
    print(
        f"  Opportunity before/after: {total_before / 1e3:.0f}k -> "
        f"{total_after / 1e3:.0f}k mm2 (delta {total_delta / 1e3:.0f}k)"
    )
    print(f"  Max shift:              {max_shift:.1f}mm")
    for i, row in enumerate(ranked[:5], start=1):
        print(
            f"  rank {i}: seed={row['seed']}, moves={row['group_moves']}, "
            f"parts={row['group_parts_moved']}, closed={row['group_closed_area_mm2'] / 1e3:.0f}k"
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
