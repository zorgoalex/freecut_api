"""V24 research: simulate adaptive V22 profile-pool policies.

V22 improves waste-region counts by always adding zp=0.4 to the profile pool.
This script uses saved V20 and V22 30-seed summaries to estimate which adaptive
policy would keep the useful V22 wins while reducing extra candidates and
rejecting bad corner tradeoffs.
"""

import csv
import json
import os
import statistics
import sys
from pathlib import Path
from typing import Callable

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
V20_DIR = ARTIFACT_ROOT / "best_layouts_v20_profile_pool_zp02_guard08_30sweep"
V22_DIR = ARTIFACT_ROOT / "best_layouts_v22_profile_pool_zp04_guard08_30sweep"
OUT_DIR = ARTIFACT_ROOT / "best_layouts_v24_adaptive_v22_policy_sim"
OUT_DIR.mkdir(parents=True, exist_ok=True)


def load_rows(directory: Path) -> list[dict]:
    path = directory / "v17b_profile_pool_service_summary.json"
    with path.open(encoding="utf-8") as handle:
        return list(json.load(handle)["results"])


def avg(values: list[float]) -> float:
    return round(statistics.fmean(values), 3) if values else 0.0


def row_metrics(rows: list[dict], candidates_completed: int) -> dict:
    zones = [int(row["n_waste_regions"]) for row in rows]
    corners = [int(row["max_corner_mm2"]) for row in rows]
    leads = [float(row["lead_util"]) for row in rows]
    return {
        "avg_zones": avg(zones),
        "zones_le_4": sum(1 for value in zones if value <= 4),
        "zones_le_5": sum(1 for value in zones if value <= 5),
        "zones_gt_5": sum(1 for value in zones if value > 5),
        "avg_lead_util": avg(leads),
        "min_lead_util": min(leads),
        "avg_max_corner_mm2": round(avg(corners)),
        "min_max_corner_mm2": min(corners),
        "corner_ge_300k": sum(1 for value in corners if value >= 300_000),
        "corner_ge_400k": sum(1 for value in corners if value >= 400_000),
        "candidates_completed": candidates_completed,
        "extra_candidates_vs_v20": candidates_completed - 90,
        "saved_candidates_vs_v22": 120 - candidates_completed,
    }


def clone_with_source(row: dict, source: str) -> dict:
    cloned = dict(row)
    cloned["policy_source"] = source
    return cloned


Policy = Callable[[dict, dict], tuple[bool, bool]]


def simulate_policy(
    name: str,
    description: str,
    v20_rows: list[dict],
    v22_rows: list[dict],
    policy: Policy,
) -> dict:
    v22_by_seed = {int(row["seed"]): row for row in v22_rows}
    chosen = []
    triggered = []
    accepted = []
    rejected = []
    for old in v20_rows:
        seed = int(old["seed"])
        new = v22_by_seed[seed]
        trigger, accept = policy(old, new)
        if trigger:
            triggered.append(seed)
        if trigger and accept:
            accepted.append(seed)
            chosen.append(clone_with_source(new, "v22_rescue"))
        else:
            if trigger:
                rejected.append(seed)
            chosen.append(clone_with_source(old, "v20_base"))

    candidates_completed = 90 + len(triggered)
    if name == "v22_full":
        candidates_completed = 120
    if name == "v20_only":
        candidates_completed = 90

    metrics = row_metrics(chosen, candidates_completed)
    return {
        "name": name,
        "description": description,
        "metrics": metrics,
        "triggered_seeds": triggered,
        "accepted_seeds": accepted,
        "rejected_triggered_seeds": rejected,
    }


def improves_zones(old: dict, new: dict) -> bool:
    return int(new["n_waste_regions"]) < int(old["n_waste_regions"])


def lead_delta(old: dict, new: dict) -> float:
    return round(float(new["lead_util"]) - float(old["lead_util"]), 3)


def corner(new: dict) -> int:
    return int(new["max_corner_mm2"])


def policy_v20_only(old: dict, new: dict) -> tuple[bool, bool]:
    return False, False


def policy_v22_full(old: dict, new: dict) -> tuple[bool, bool]:
    return True, True


def policy_gt5_only(old: dict, new: dict) -> tuple[bool, bool]:
    trigger = int(old["n_waste_regions"]) > 5
    return trigger, trigger and improves_zones(old, new)


def policy_z5_corner300(old: dict, new: dict) -> tuple[bool, bool]:
    trigger = int(old["n_waste_regions"]) >= 5
    accept = improves_zones(old, new) and corner(new) >= 300_000
    return trigger, accept


def policy_z5_corner300_lead08(old: dict, new: dict) -> tuple[bool, bool]:
    trigger = int(old["n_waste_regions"]) >= 5
    accept = (
        improves_zones(old, new)
        and corner(new) >= 300_000
        and lead_delta(old, new) >= -0.8
    )
    return trigger, accept


def policy_z5_corner300_lead04(old: dict, new: dict) -> tuple[bool, bool]:
    trigger = int(old["n_waste_regions"]) >= 5
    accept = (
        improves_zones(old, new)
        and corner(new) >= 300_000
        and lead_delta(old, new) >= -0.4
    )
    return trigger, accept


def policy_z5_zones_only(old: dict, new: dict) -> tuple[bool, bool]:
    trigger = int(old["n_waste_regions"]) >= 5
    return trigger, improves_zones(old, new)


POLICIES: list[tuple[str, str, Policy]] = [
    ("v20_only", "baseline V20 pool [0.2,0.3,0.5]", policy_v20_only),
    ("v22_full", "full V22 pool [0.2,0.3,0.4,0.5]", policy_v22_full),
    (
        "gt5_only",
        "run zp=0.4 only when V20 leaves >5 zones; accept any zone improvement",
        policy_gt5_only,
    ),
    (
        "z5_corner300",
        "run zp=0.4 when V20 has >=5 zones; accept only zone improvement with corner >=300k",
        policy_z5_corner300,
    ),
    (
        "z5_corner300_lead08",
        "same as z5_corner300, but reject if lead drops more than 0.8pp vs V20",
        policy_z5_corner300_lead08,
    ),
    (
        "z5_corner300_lead04",
        "same as z5_corner300, but reject if lead drops more than 0.4pp vs V20",
        policy_z5_corner300_lead04,
    ),
    (
        "z5_zones_only",
        "run zp=0.4 when V20 has >=5 zones; accept any zone improvement",
        policy_z5_zones_only,
    ),
]


def write_csv(path: Path, policies: list[dict]) -> None:
    fields = [
        "name",
        "avg_zones",
        "zones_le_4",
        "zones_le_5",
        "zones_gt_5",
        "avg_lead_util",
        "avg_max_corner_mm2",
        "corner_ge_300k",
        "candidates_completed",
        "extra_candidates_vs_v20",
        "saved_candidates_vs_v22",
        "triggered_seeds",
        "accepted_seeds",
    ]
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for item in policies:
            row = {"name": item["name"]}
            row.update(
                {
                    key: value
                    for key, value in item["metrics"].items()
                    if key in fields
                }
            )
            row["triggered_seeds"] = " ".join(map(str, item["triggered_seeds"]))
            row["accepted_seeds"] = " ".join(map(str, item["accepted_seeds"]))
            writer.writerow(row)


def self_check(policies: list[dict]) -> None:
    by_name = {item["name"]: item for item in policies}
    assert by_name["v20_only"]["metrics"]["avg_zones"] == 4.833
    assert by_name["v22_full"]["metrics"]["avg_zones"] == 4.633
    assert by_name["gt5_only"]["accepted_seeds"] == [4]
    assert by_name["z5_corner300"]["accepted_seeds"] == [2, 4, 9, 14]


def main() -> None:
    v20_rows = load_rows(V20_DIR)
    v22_rows = load_rows(V22_DIR)
    policies = [
        simulate_policy(name, description, v20_rows, v22_rows, policy)
        for name, description, policy in POLICIES
    ]
    self_check(policies)

    output = {
        "inputs": {
            "artifact_root": str(ARTIFACT_ROOT),
            "v20_dir": str(V20_DIR),
            "v22_dir": str(V22_DIR),
        },
        "policies": policies,
        "recommendation": {
            "default": "gt5_only",
            "balanced_breakthrough": "z5_corner300",
            "reason": (
                "gt5_only fixes the only >5-zone outlier with one extra candidate; "
                "z5_corner300 keeps the breakthrough threshold (<=4 zones on 10/30) "
                "while rejecting the worst corner regression seed."
            ),
        },
    }
    with (OUT_DIR / "v24_adaptive_v22_policy_summary.json").open(
        "w", encoding="utf-8"
    ) as handle:
        json.dump(output, handle, ensure_ascii=False, indent=2)
    write_csv(OUT_DIR / "v24_adaptive_v22_policy_summary.csv", policies)

    print("V24 adaptive V22 policy simulation")
    for item in policies:
        metrics = item["metrics"]
        print(
            f"{item['name']}: avg_zones={metrics['avg_zones']}, "
            f"<=4={metrics['zones_le_4']}/30, <=5={metrics['zones_le_5']}/30, "
            f"lead={metrics['avg_lead_util']}%, corner={metrics['avg_max_corner_mm2']}, "
            f"candidates={metrics['candidates_completed']}, accepted={item['accepted_seeds']}"
        )
    print(f"summary={OUT_DIR / 'v24_adaptive_v22_policy_summary.json'}")
    print(f"csv={OUT_DIR / 'v24_adaptive_v22_policy_summary.csv'}")


if __name__ == "__main__":
    main()
