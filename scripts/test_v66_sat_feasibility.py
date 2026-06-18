#!/usr/bin/env python3
"""V66: SAT/MaxSAT/exact-cover feasibility estimator.

This script does not implement a SAT solver. It estimates the size of common
rectangle-packing encodings for Freecut fixtures so we can decide whether a
SAT/MaxSAT/column-generation direction is realistic before spending effort on a
full engine.
"""

from __future__ import annotations

import argparse
import json
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def find_artifact_root() -> Path:
    current = Path(__file__).resolve()
    marker = Path("ai_docs") / "tmp"
    for parent in current.parents:
        if (parent / marker).exists() and parent.name == "freecut_repo":
            return parent
    for parent in current.parents:
        if (parent / marker).exists():
            return parent
    return ROOT


ARTIFACT_ROOT = find_artifact_root()


@dataclass(frozen=True)
class Piece:
    id: str
    w: float
    h: float
    allow_rotate: bool

    @property
    def area(self) -> float:
        return self.w * self.h


def parse_steps(value: str) -> list[float]:
    return [float(part.strip()) for part in value.split(",") if part.strip()]


def load_fixture(path: Path, repeat_factor: int) -> dict[str, Any]:
    req = json.loads(path.read_text(encoding="utf-8"))
    if repeat_factor > 1:
        for item in req["items"]:
            item["qty"] = int(item["qty"]) * repeat_factor
    return req


def expand_pieces(req: dict[str, Any]) -> list[Piece]:
    pieces: list[Piece] = []
    for item in req["items"]:
        for _copy_idx in range(int(item["qty"])):
            pieces.append(
                Piece(
                    id=item["id"],
                    w=float(item["width_mm"]),
                    h=float(item["height_mm"]),
                    allow_rotate=item.get("rotation", "allow_90") == "allow_90",
                )
            )
    return pieces


def usable_size(req: dict[str, Any]) -> tuple[float, float]:
    stock = req["stock"][0]
    trim = req["params"]["trim_mm"]
    return (
        float(stock["width_mm"]) - float(trim["left"]) - float(trim["right"]),
        float(stock["height_mm"]) - float(trim["top"]) - float(trim["bottom"]),
    )


def orientations(piece: Piece) -> list[tuple[float, float]]:
    out = [(piece.w, piece.h)]
    if piece.allow_rotate and abs(piece.w - piece.h) > 1e-9:
        out.append((piece.h, piece.w))
    return out


def start_count(usable: float, size: float, step: float) -> int:
    if size > usable + 1e-9:
        return 0
    return math.floor((usable - size) / step + 1e-9) + 1


def candidate_count_for_piece(piece: Piece, usable_w: float, usable_h: float, step: float) -> int:
    total = 0
    for w, h in orientations(piece):
        total += start_count(usable_w, w, step) * start_count(usable_h, h, step)
    return total


def estimate_step(
    pieces: list[Piece],
    usable_w: float,
    usable_h: float,
    sheet_count: int,
    step: float,
) -> dict[str, Any]:
    per_piece = [candidate_count_for_piece(piece, usable_w, usable_h, step) for piece in pieces]
    candidates_per_sheet = sum(per_piece)
    total_candidates = candidates_per_sheet * sheet_count

    pair_upper_per_sheet = 0
    prefix_sum = 0
    for count in per_piece:
        pair_upper_per_sheet += prefix_sum * count
        prefix_sum += count
    total_pair_upper = pair_upper_per_sheet * sheet_count

    cells_x = math.ceil(usable_w / step)
    cells_y = math.ceil(usable_h / step)
    sheet_cells = cells_x * cells_y
    total_cells = sheet_cells * sheet_count

    return {
        "grid_step_mm": step,
        "candidate_vars_total": total_candidates,
        "candidate_vars_per_sheet": candidates_per_sheet,
        "candidate_vars_min_per_piece_per_sheet": min(per_piece) if per_piece else 0,
        "candidate_vars_max_per_piece_per_sheet": max(per_piece) if per_piece else 0,
        "candidate_vars_avg_per_piece_per_sheet": (sum(per_piece) / len(per_piece)) if per_piece else 0.0,
        "item_exactly_one_constraints": len(pieces),
        "same_sheet_candidate_pair_upper_bound": total_pair_upper,
        "grid_cells_per_sheet": sheet_cells,
        "grid_cells_total": total_cells,
        "cell_occupancy_at_most_one_constraints": total_cells,
        "notes": classify_step(step, total_candidates, total_pair_upper),
    }


def classify_step(step: float, candidates: int, pair_upper: int) -> str:
    if step <= 1.0:
        return "near-exact grid: combinatorial size is impractical for direct SAT/exact-cover"
    if candidates > 5_000_000 or pair_upper > 10_000_000_000:
        return "too large for naive SAT; requires aggressive candidate pruning/column generation"
    if candidates > 500_000:
        return "large; maybe offline only with pruning and a strong incumbent"
    if step >= 50.0:
        return "coarse benchmark only; geometry quality is not exact enough for production"
    return "borderline; useful for feasibility experiments, not final precision"


def lower_bound_sheets(pieces: list[Piece], usable_w: float, usable_h: float) -> int:
    return math.ceil(sum(piece.area for piece in pieces) / (usable_w * usable_h))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fixture",
        type=Path,
        default=ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json",
    )
    parser.add_argument("--repeat-factor", type=int, default=1)
    parser.add_argument("--sheet-count", type=int)
    parser.add_argument("--steps-mm", default="100,50,25,10,5,1,0.5")
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=ARTIFACT_ROOT / "ai_docs" / "tmp" / "v66_sat_feasibility",
    )
    args = parser.parse_args()

    req = load_fixture(args.fixture, args.repeat_factor)
    pieces = expand_pieces(req)
    usable_w, usable_h = usable_size(req)
    lower_bound = lower_bound_sheets(pieces, usable_w, usable_h)
    sheet_count = args.sheet_count or lower_bound
    gap = float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"])
    steps = parse_steps(args.steps_mm)
    summary = {
        "fixture": str(args.fixture),
        "repeat_factor": args.repeat_factor,
        "piece_count": len(pieces),
        "usable_w_mm": usable_w,
        "usable_h_mm": usable_h,
        "gap_mm": gap,
        "lower_bound_sheets_by_area": lower_bound,
        "sheet_count": sheet_count,
        "important_note": (
            "Exact geometry for this fixture needs sub-mm/0.5mm-scale reasoning because "
            "kerf+spacing can be fractional. Coarse grids are only feasibility proxies."
        ),
        "steps": [estimate_step(pieces, usable_w, usable_h, sheet_count, step) for step in steps],
    }
    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    print(f"fixture={args.fixture} pieces={len(pieces)} sheets={sheet_count} lower_bound={lower_bound}")
    print("step_mm,candidate_vars,pair_upper_bound,grid_cells_total,note")
    for row in summary["steps"]:
        print(
            f"{row['grid_step_mm']},"
            f"{row['candidate_vars_total']},"
            f"{row['same_sheet_candidate_pair_upper_bound']},"
            f"{row['grid_cells_total']},"
            f"{row['notes']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
