"""V47: compare visual0 and cut-gap waste-region metrics on saved layouts.

The API response stores placement coordinates in usable-area space.  This
script intentionally does not subtract trim from placement x/y again.
"""

from __future__ import annotations

import argparse
import csv
import json
from collections import deque
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "ai_docs" / "tmp" / "v47_dual_gap_visual_benchmark"
CELL_MM = 10.0
MIN_REGION_CELLS = 50

CASES = [
    (
        "v41c_seed11_old",
        ROOT / "ai_docs" / "tmp" / "v41c_visual_artifacts" / "seed_11_old.json",
    ),
    (
        "v41c_seed11_new",
        ROOT / "ai_docs" / "tmp" / "v41c_visual_artifacts" / "seed_11_new.json",
    ),
    (
        "v41c_seed13_old",
        ROOT / "ai_docs" / "tmp" / "v41c_visual_artifacts" / "seed_13_old.json",
    ),
    (
        "v41c_seed13_new",
        ROOT / "ai_docs" / "tmp" / "v41c_visual_artifacts" / "seed_13_new.json",
    ),
    (
        "v43_seed13_hard_guard",
        ROOT / "ai_docs" / "tmp" / "v43_hard_lead_guard" / "seed_13_v43_hard_guard.json",
    ),
    (
        "v31_seed08_group_shift",
        ROOT
        / "ai_docs"
        / "tmp"
        / "best_layouts_v31_paired_group_shift_diff_quick"
        / "seed_08_moves4_closed498280.json",
    ),
    (
        "v30_seed2_group_shift_off",
        ROOT / "ai_docs" / "tmp" / "best_layouts_v30_group_shift_metrics_quick" / "off_seed2.json",
    ),
    (
        "v30_seed2_group_shift_on",
        ROOT / "ai_docs" / "tmp" / "best_layouts_v30_group_shift_metrics_quick" / "on_seed2.json",
    ),
]


def load_response(path: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return raw.get("response", raw)


def waste_regions_for_solution(solution: dict[str, Any], gap_mm: float) -> int:
    trim = solution.get("trim_mm") or {}
    width = float(solution["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    height = float(solution["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    nx = int(width // CELL_MM)
    ny = int(height // CELL_MM)
    if nx <= 0 or ny <= 0:
        return 0

    occ = [[False for _ in range(nx)] for _ in range(ny)]
    for placement in solution.get("placements", []):
        x0 = max(0.0, float(placement["x_mm"]) - gap_mm)
        y0 = max(0.0, float(placement["y_mm"]) - gap_mm)
        x1 = min(width, float(placement["x_mm"]) + float(placement["width_mm"]) + gap_mm)
        y1 = min(height, float(placement["y_mm"]) + float(placement["height_mm"]) + gap_mm)
        if x1 <= x0 or y1 <= y0:
            continue
        i0 = int(x0 // CELL_MM)
        j0 = int(y0 // CELL_MM)
        i1 = min(nx - 1, int((x1 - 1e-9) // CELL_MM))
        j1 = min(ny - 1, int((y1 - 1e-9) // CELL_MM))
        for j in range(j0, j1 + 1):
            for i in range(i0, i1 + 1):
                occ[j][i] = True

    seen = [[False for _ in range(nx)] for _ in range(ny)]
    regions = 0
    for start_j in range(ny):
        for start_i in range(nx):
            if occ[start_j][start_i] or seen[start_j][start_i]:
                continue
            cells = 0
            queue: deque[tuple[int, int]] = deque([(start_i, start_j)])
            seen[start_j][start_i] = True
            while queue:
                i, j = queue.pop()
                cells += 1
                for ni, nj in ((i - 1, j), (i + 1, j), (i, j - 1), (i, j + 1)):
                    if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                        seen[nj][ni] = True
                        queue.append((ni, nj))
            if cells >= MIN_REGION_CELLS:
                regions += 1
    return regions


def sheet_util_pct(solution: dict[str, Any]) -> float:
    trim = solution.get("trim_mm") or {}
    width = float(solution["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    height = float(solution["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    area = width * height
    if area <= 0:
        return 0.0
    used = sum(float(p["width_mm"]) * float(p["height_mm"]) for p in solution.get("placements", []))
    return used / area * 100.0


def lead_util_pct(utils: list[float]) -> float:
    if not utils:
        return 0.0
    ordered = sorted(utils, reverse=True)
    if len(ordered) == 1:
        return ordered[0]
    return sum(ordered[:-1]) / (len(ordered) - 1)


def summarize_case(name: str, path: Path, cut_gap_mm: float) -> dict[str, Any] | None:
    if not path.exists():
        return None
    response = load_response(path)
    solutions = response.get("solutions", [])
    visual0_by_sheet = [waste_regions_for_solution(solution, 0.0) for solution in solutions]
    cut_gap_by_sheet = [waste_regions_for_solution(solution, cut_gap_mm) for solution in solutions]
    utils = [sheet_util_pct(solution) for solution in solutions]
    profile_pool = response.get("summary", {}).get("profile_pool") or {}
    group_shift = response.get("summary", {}).get("group_shift") or {}
    return {
        "case": name,
        "path": str(path.relative_to(ROOT)),
        "sheets": len(solutions),
        "placements": sum(len(solution.get("placements", [])) for solution in solutions),
        "zones_visual0": sum(visual0_by_sheet),
        "zones_cut_gap": sum(cut_gap_by_sheet),
        "zones_delta_cut_minus_visual": sum(cut_gap_by_sheet) - sum(visual0_by_sheet),
        "per_sheet_visual0": visual0_by_sheet,
        "per_sheet_cut_gap": cut_gap_by_sheet,
        "lead_util_pct": round(lead_util_pct(utils), 4),
        "min_util_pct": round(min(utils), 4) if utils else 0.0,
        "profile_pool_winner_zones": profile_pool.get("winner_waste_regions"),
        "profile_pool_winner_visual_zones": profile_pool.get("winner_visual_waste_regions"),
        "group_shift_moves": group_shift.get("moves_applied"),
        "group_shift_closed_area_mm2": group_shift.get("corridor_closed_area_mm2"),
    }


def write_outputs(rows: list[dict[str, Any]], cut_gap_mm: float) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    (OUT_DIR / "v47_dual_gap_metrics.json").write_text(
        json.dumps({"cut_gap_mm": cut_gap_mm, "rows": rows}, indent=2),
        encoding="utf-8",
    )
    csv_fields = [
        "case",
        "sheets",
        "placements",
        "zones_visual0",
        "zones_cut_gap",
        "zones_delta_cut_minus_visual",
        "lead_util_pct",
        "min_util_pct",
        "profile_pool_winner_zones",
        "profile_pool_winner_visual_zones",
        "group_shift_moves",
        "group_shift_closed_area_mm2",
        "path",
    ]
    with (OUT_DIR / "v47_dual_gap_metrics.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=csv_fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field) for field in csv_fields})

    differing = [row for row in rows if row["zones_visual0"] != row["zones_cut_gap"]]
    lines = [
        "# V47 dual-gap visual benchmark",
        "",
        f"cut_gap_mm: {cut_gap_mm}",
        f"cases: {len(rows)}",
        f"gap-sensitive cases: {len(differing)}",
        "",
        "| case | visual0 | cut_gap | delta | lead % | min % |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row['case']} | {row['zones_visual0']} | {row['zones_cut_gap']} | "
            f"{row['zones_delta_cut_minus_visual']} | {row['lead_util_pct']} | {row['min_util_pct']} |"
        )
    lines.extend(
        [
            "",
            "Conclusion: use both `zones_visual0` and `zones_cut_gap` in future paired audits. "
            "When they diverge, automatic scoring must not replace visual review.",
        ]
    )
    (OUT_DIR / "v47_summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cut-gap-mm", type=float, default=7.0)
    args = parser.parse_args()
    rows = [
        row
        for row in (summarize_case(name, path, args.cut_gap_mm) for name, path in CASES)
        if row is not None
    ]
    if not rows:
        raise SystemExit("No benchmark artifacts found")
    write_outputs(rows, args.cut_gap_mm)
    print(f"wrote {len(rows)} rows to {OUT_DIR}")


if __name__ == "__main__":
    main()
