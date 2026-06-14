"""V26 research: empirical service sweeps for V25 adaptive profile rescue.

Runs the existing V17b profile-pool service benchmark twice against an already
running Freecut service:

1. `gt5_only`: base V20 pool plus zp=0.4 rescue only when zones > 5.
2. `z5_corner300`: base V20 pool plus zp=0.4 rescue when zones > 4, accepting
   rescue candidates only when their largest reusable corner is at least 300k.

The script stores raw benchmark outputs under `ai_docs/tmp` and writes a compact
combined summary for comparing against V20/V22/V24.
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = Path(__file__).resolve().parents[1]


def default_artifact_root() -> Path:
    parts = ROOT.parts
    lowered = [part.lower() for part in parts]
    for index in range(len(lowered) - 2):
        if (
            lowered[index] == "ai_docs"
            and lowered[index + 1] == "tmp"
            and lowered[index + 2] == "worktrees"
        ):
            return Path(*parts[: index + 2])
    return ROOT / "ai_docs" / "tmp"


ARTIFACT_ROOT = Path(os.environ.get("FREECUT_ARTIFACT_ROOT", default_artifact_root()))
OUT_DIR = ARTIFACT_ROOT / "best_layouts_v26_v25_adaptive_sweeps"
OUT_DIR.mkdir(parents=True, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8090")
SEEDS = os.environ.get("FREECUT_SEEDS", "30")
EXPECTED_SEEDS = int(SEEDS)
TIME_LIMIT_MS = os.environ.get("FREECUT_TIME_LIMIT_MS", "10000")
SHEET_BUDGET_MS = os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000")

CONFIGS = [
    {
        "name": "gt5_only",
        "profiles": "0.2,0.3,0.5",
        "rescue_profiles": "0.4",
        "rescue_zones_gt": "5",
        "rescue_accept_min_corner": "",
        "description": "Run zp=0.4 only if V20 provisional winner has >5 waste regions.",
    },
    {
        "name": "z5_corner300",
        "profiles": "0.2,0.3,0.5",
        "rescue_profiles": "0.4",
        "rescue_zones_gt": "4",
        "rescue_accept_min_corner": "300000",
        "description": "Run zp=0.4 for 5+ waste regions, but reject rescue corners below 300k.",
    },
]


def aggregate(rows: list[dict]) -> dict:
    n = max(1, len(rows))
    zones = [int(row["n_waste_regions"]) for row in rows]
    leads = [float(row["lead_util"]) for row in rows]
    corners = [float(row["max_corner_mm2"]) for row in rows]
    completed = [int(row.get("candidates_completed", 0)) for row in rows]
    rescue = [row for row in rows if row.get("rescue_triggered")]
    guard_rejected = [
        int(row.get("rescue_candidates_rejected_by_guard", 0)) for row in rows
    ]
    return {
        "count": len(rows),
        "four_sheet_count": sum(1 for row in rows if int(row["sheets"]) == 4),
        "avg_zones": round(sum(zones) / n, 3),
        "zones_le_4": sum(1 for value in zones if value <= 4),
        "zones_le_5": sum(1 for value in zones if value <= 5),
        "zones_gt_5": sum(1 for value in zones if value > 5),
        "avg_lead_util": round(sum(leads) / n, 3),
        "min_lead_util": min(leads) if leads else 0.0,
        "avg_max_corner_mm2": round(sum(corners) / n) if corners else 0,
        "min_max_corner_mm2": round(min(corners)) if corners else 0,
        "corner_ge_300k": sum(1 for value in corners if value >= 300_000),
        "corner_ge_400k": sum(1 for value in corners if value >= 400_000),
        "avg_candidates_completed": round(sum(completed) / n, 3),
        "total_candidates_completed": sum(completed),
        "rescue_triggered_count": len(rescue),
        "rescue_triggered_seeds": [int(row["seed"]) for row in rescue],
        "guard_rejected_total": sum(guard_rejected),
    }


def run_config(config: dict) -> dict:
    config_dir = OUT_DIR / config["name"]
    config_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(
        {
            "FREECUT_PORT": PORT,
            "FREECUT_SEEDS": SEEDS,
            "FREECUT_TIME_LIMIT_MS": TIME_LIMIT_MS,
            "FREECUT_SHEET_BUDGET_MS": SHEET_BUDGET_MS,
            "FREECUT_PROFILE_POOL": config["profiles"],
            "FREECUT_PROFILE_POOL_RESCUE_PROFILES": config["rescue_profiles"],
            "FREECUT_PROFILE_POOL_RESCUE_ZONES_GT": config["rescue_zones_gt"],
            "FREECUT_PROFILE_POOL_RESCUE_ACCEPT_MIN_CORNER_MM2": config[
                "rescue_accept_min_corner"
            ],
            "FREECUT_PROFILE_POOL_MAX_LEAD_DROP_PP": "0.8",
            "FREECUT_OUT_DIR": str(config_dir),
        }
    )
    print(f"\n=== Running {config['name']} ===", flush=True)
    started = time.time()
    subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "test_v17b_profile_pool_service.py")],
        cwd=ROOT,
        env=env,
        check=True,
    )
    summary_path = config_dir / "v17b_profile_pool_service_summary.json"
    with summary_path.open(encoding="utf-8") as handle:
        summary = json.load(handle)
    rows = summary.get("results", [])
    if len(rows) != EXPECTED_SEEDS:
        raise RuntimeError(
            f"{config['name']} produced {len(rows)} rows, expected {EXPECTED_SEEDS}"
        )
    return {
        "name": config["name"],
        "description": config["description"],
        "dir": str(config_dir),
        "elapsed_s": round(time.time() - started, 1),
        "aggregate": aggregate(rows),
        "summary_path": str(summary_path),
    }


def main() -> None:
    started = time.time()
    results = []
    print(
        f"V26 empirical adaptive sweeps: port={PORT}, seeds={SEEDS}, "
        f"time_limit_ms={TIME_LIMIT_MS}, sheet_budget_ms={SHEET_BUDGET_MS}",
        flush=True,
    )
    for config in CONFIGS:
        results.append(run_config(config))
    output = {
        "artifact_root": str(ARTIFACT_ROOT),
        "out_dir": str(OUT_DIR),
        "port": PORT,
        "seeds": int(SEEDS),
        "time_limit_ms": int(TIME_LIMIT_MS),
        "sheet_budget_ms": int(SHEET_BUDGET_MS),
        "elapsed_s": round(time.time() - started, 1),
        "results": results,
    }
    combined_path = OUT_DIR / "v26_v25_adaptive_sweeps_summary.json"
    with combined_path.open("w", encoding="utf-8") as handle:
        json.dump(output, handle, ensure_ascii=False, indent=2)

    print("\nV26 combined summary")
    for result in results:
        aggregate_ = result["aggregate"]
        print(
            f"{result['name']}: avg_zones={aggregate_['avg_zones']}, "
            f"<=4={aggregate_['zones_le_4']}/{EXPECTED_SEEDS}, "
            f"<=5={aggregate_['zones_le_5']}/{EXPECTED_SEEDS}, "
            f"lead={aggregate_['avg_lead_util']}%, corner={aggregate_['avg_max_corner_mm2']}, "
            f"candidates={aggregate_['total_candidates_completed']}, "
            f"rescue={aggregate_['rescue_triggered_seeds']}",
            flush=True,
        )
    print(f"summary={combined_path}", flush=True)


if __name__ == "__main__":
    main()
