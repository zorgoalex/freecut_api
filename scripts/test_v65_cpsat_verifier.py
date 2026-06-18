#!/usr/bin/env python3
"""V65: OR-Tools CP-SAT fixed-sheet verifier benchmark.

This script checks whether a Freecut fixture can be packed into a fixed number
of sheets using CP-SAT NoOverlap2D. It is intended as an exact/near-exact
research verifier for small and medium instances, not as a latency-critical
production engine.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def find_repo_root() -> Path:
    current = Path(__file__).resolve()
    for parent in current.parents:
        if (parent / "Cargo.toml").exists() and parent.name == "freecut_repo":
            return parent
    for parent in current.parents:
        if (parent / "Cargo.toml").exists():
            return parent
    return ROOT


def find_artifact_root() -> Path:
    repo = find_repo_root()
    if (repo / "ai_docs" / "tmp").exists():
        return repo
    for parent in Path(__file__).resolve().parents:
        if (parent / "ai_docs" / "tmp").exists():
            return parent
    return repo


def add_local_ortools_path() -> None:
    local_deps = find_artifact_root() / "ai_docs" / "tmp" / "pydeps" / "ortools"
    if local_deps.exists():
        sys.path.insert(0, str(local_deps))


add_local_ortools_path()

try:
    from ortools.sat.python import cp_model
except ModuleNotFoundError as exc:
    raise SystemExit(
        "OR-Tools is not installed. Install locally with: "
        "python -m pip install --target ai_docs/tmp/pydeps/ortools ortools"
    ) from exc


@dataclass(frozen=True)
class Piece:
    id: str
    type_idx: int
    copy_idx: int
    w: int
    h: int
    allow_rotate: bool

    @property
    def area(self) -> int:
        return self.w * self.h


@dataclass(frozen=True)
class Placement:
    piece: Piece
    sheet_idx: int
    x: int
    y: int
    w: int
    h: int
    rotated: bool


@dataclass(frozen=True)
class HintPlacement:
    piece_idx: int
    sheet_idx: int
    x: int
    y: int
    w: int
    h: int
    rotated: bool


def scaled(value: float, scale: int) -> int:
    return int(round(value * scale))


def load_fixture(path: Path, repeat_factor: int) -> dict[str, Any]:
    req = json.loads(path.read_text(encoding="utf-8"))
    if repeat_factor > 1:
        for item in req["items"]:
            item["qty"] = int(item["qty"]) * repeat_factor
    return req


def usable_size(req: dict[str, Any], scale: int) -> tuple[int, int]:
    stock = req["stock"][0]
    trim = req["params"]["trim_mm"]
    return (
        scaled(float(stock["width_mm"]) - float(trim["left"]) - float(trim["right"]), scale),
        scaled(float(stock["height_mm"]) - float(trim["top"]) - float(trim["bottom"]), scale),
    )


def expand_pieces(req: dict[str, Any], scale: int) -> list[Piece]:
    pieces: list[Piece] = []
    for type_idx, item in enumerate(req["items"]):
        for copy_idx in range(int(item["qty"])):
            pieces.append(
                Piece(
                    id=item["id"],
                    type_idx=type_idx,
                    copy_idx=copy_idx,
                    w=scaled(float(item["width_mm"]), scale),
                    h=scaled(float(item["height_mm"]), scale),
                    allow_rotate=item.get("rotation", "allow_90") == "allow_90",
                )
            )
    return pieces


def lower_bound_sheets(pieces: list[Piece], usable_w: int, usable_h: int) -> int:
    return math.ceil(sum(piece.area for piece in pieces) / (usable_w * usable_h))


def orientations(piece: Piece) -> list[tuple[int, int, bool]]:
    out = [(piece.w, piece.h, False)]
    if piece.allow_rotate and piece.w != piece.h:
        out.append((piece.h, piece.w, True))
    return out


def attrs_from_tag(tag: str) -> dict[str, str]:
    return dict(re.findall(r'([a-zA-Z_-]+)="([^"]*)"', tag))


def parse_hint_svg(req: dict[str, Any], pieces: list[Piece], scale: int, hint_svg: Path) -> dict[int, HintPlacement]:
    text = hint_svg.read_text(encoding="utf-8")
    stock = req["stock"][0]
    sheet_h = float(stock["height_mm"])
    trim = req["params"]["trim_mm"]
    gap_y = 50.0
    used_piece_indices: set[int] = set()
    hints: dict[int, HintPlacement] = {}
    rect_text_pattern = re.compile(r"<rect\s+([^>]*)/>\s*<text\s+[^>]*>([^<]*)</text>", re.MULTILINE)
    for match in rect_text_pattern.finditer(text):
        rect_attrs = attrs_from_tag(match.group(1))
        if rect_attrs.get("stroke") != "#9a1515":
            continue
        piece_id = match.group(2)
        x_mm = float(rect_attrs["x"])
        y_mm = float(rect_attrs["y"])
        w = scaled(float(rect_attrs["width"]), scale)
        h = scaled(float(rect_attrs["height"]), scale)
        sheet_idx = int(y_mm // (sheet_h + gap_y))
        sheet_y0 = sheet_idx * (sheet_h + gap_y)
        local_x = scaled(x_mm - float(trim["left"]), scale)
        local_y = scaled(y_mm - sheet_y0 - float(trim["top"]), scale)
        matched_piece_idx = None
        matched_rotated = False
        for piece_idx, piece in enumerate(pieces):
            if piece_idx in used_piece_indices or piece.id != piece_id:
                continue
            if piece.w == w and piece.h == h:
                matched_piece_idx = piece_idx
                matched_rotated = False
                break
            if piece.allow_rotate and piece.w == h and piece.h == w:
                matched_piece_idx = piece_idx
                matched_rotated = True
                break
        if matched_piece_idx is None:
            continue
        used_piece_indices.add(matched_piece_idx)
        hints[matched_piece_idx] = HintPlacement(
            piece_idx=matched_piece_idx,
            sheet_idx=sheet_idx,
            x=local_x,
            y=local_y,
            w=w,
            h=h,
            rotated=matched_rotated,
        )
    return hints


def solve_fixed_sheet_count(
    pieces: list[Piece],
    usable_w: int,
    usable_h: int,
    gap: int,
    sheet_count: int,
    time_limit_s: float,
    workers: int,
    objective: str,
    hints: dict[int, HintPlacement],
    fix_hint: bool,
) -> tuple[dict[str, Any], list[Placement]]:
    model = cp_model.CpModel()
    x_vars = []
    y_vars = []
    option_vars: dict[tuple[int, int, bool, int], Any] = {}
    option_dims: dict[tuple[int, int, bool, int], tuple[int, int]] = {}
    x_intervals: list[list[Any]] = [[] for _ in range(sheet_count)]
    y_intervals: list[list[Any]] = [[] for _ in range(sheet_count)]

    for piece_idx, piece in enumerate(pieces):
        x_var = model.NewIntVar(0, usable_w, f"x_{piece_idx}")
        y_var = model.NewIntVar(0, usable_h, f"y_{piece_idx}")
        x_vars.append(x_var)
        y_vars.append(y_var)
        piece_options = []
        for sheet_idx in range(sheet_count):
            for actual_w, actual_h, rotated in orientations(piece):
                if actual_w > usable_w or actual_h > usable_h:
                    continue
                option_idx = len(piece_options)
                present = model.NewBoolVar(f"p_{piece_idx}_{sheet_idx}_{option_idx}")
                key = (piece_idx, sheet_idx, rotated, option_idx)
                option_vars[key] = present
                option_dims[key] = (actual_w, actual_h)
                piece_options.append(present)

                model.Add(x_var <= usable_w - actual_w).OnlyEnforceIf(present)
                model.Add(y_var <= usable_h - actual_h).OnlyEnforceIf(present)
                x_intervals[sheet_idx].append(
                    model.NewOptionalFixedSizeIntervalVar(
                        x_var,
                        actual_w + gap,
                        present,
                        f"xi_{piece_idx}_{sheet_idx}_{option_idx}",
                    )
                )
                y_intervals[sheet_idx].append(
                    model.NewOptionalFixedSizeIntervalVar(
                        y_var,
                        actual_h + gap,
                        present,
                        f"yi_{piece_idx}_{sheet_idx}_{option_idx}",
                    )
                )
        if not piece_options:
            return (
                {
                    "status": "IMPOSSIBLE_DIMENSION",
                    "feasible": False,
                    "reason": f"piece {piece.id}#{piece.copy_idx} does not fit any orientation",
                },
                [],
            )
        model.AddExactlyOne(piece_options)

    for sheet_idx in range(sheet_count):
        model.AddNoOverlap2D(x_intervals[sheet_idx], y_intervals[sheet_idx])

    hinted_options = 0
    for piece_idx, hint in hints.items():
        model.AddHint(x_vars[piece_idx], hint.x)
        model.AddHint(y_vars[piece_idx], hint.y)
        for (option_piece_idx, sheet_idx, rotated, option_idx), present in option_vars.items():
            if option_piece_idx != piece_idx:
                continue
            actual_w, actual_h = option_dims[(option_piece_idx, sheet_idx, rotated, option_idx)]
            selected = (
                sheet_idx == hint.sheet_idx
                and rotated == hint.rotated
                and actual_w == hint.w
                and actual_h == hint.h
            )
            model.AddHint(present, 1 if selected else 0)
            if selected:
                hinted_options += 1

    if objective == "anchor":
        model.Minimize(sum(x_vars) + sum(y_vars))

    solver = cp_model.CpSolver()
    solver.parameters.max_time_in_seconds = time_limit_s
    solver.parameters.num_search_workers = workers
    if hints:
        solver.parameters.repair_hint = True
        solver.parameters.hint_conflict_limit = 100000
    if hints and fix_hint:
        solver.parameters.fix_variables_to_their_hinted_value = True
    started = time.perf_counter()
    status = solver.Solve(model)
    wall_ms = (time.perf_counter() - started) * 1000.0
    status_name = solver.StatusName(status)
    feasible = status in (cp_model.OPTIMAL, cp_model.FEASIBLE)
    placements: list[Placement] = []
    if feasible:
        for (piece_idx, sheet_idx, rotated, option_idx), present in option_vars.items():
            if solver.BooleanValue(present):
                actual_w, actual_h = option_dims[(piece_idx, sheet_idx, rotated, option_idx)]
                placements.append(
                    Placement(
                        piece=pieces[piece_idx],
                        sheet_idx=sheet_idx,
                        x=solver.Value(x_vars[piece_idx]),
                        y=solver.Value(y_vars[piece_idx]),
                        w=actual_w,
                        h=actual_h,
                        rotated=rotated,
                    )
                )
    summary = {
        "status": status_name,
        "feasible": feasible,
        "objective": objective,
        "sheet_count": sheet_count,
        "piece_count": len(pieces),
        "time_limit_s": time_limit_s,
        "workers": workers,
        "solver_wall_s": solver.WallTime(),
        "script_wall_ms": wall_ms,
        "conflicts": solver.NumConflicts(),
        "branches": solver.NumBranches(),
        "objective_value": solver.ObjectiveValue() if feasible and objective != "none" else None,
        "hinted_pieces": len(hints),
        "hinted_options": hinted_options,
        "fix_hint": fix_hint,
    }
    return summary, placements


def summarize_waste(placements: list[Placement], sheet_count: int, usable_w: int, usable_h: int) -> dict[str, Any]:
    used_area = sum(p.piece.area for p in placements)
    sheet_area = usable_w * usable_h * sheet_count
    utils = []
    for sheet_idx in range(sheet_count):
        area = sum(p.piece.area for p in placements if p.sheet_idx == sheet_idx)
        utils.append(area / (usable_w * usable_h) * 100.0 if usable_w * usable_h else 0.0)
    return {
        "waste_percent": ((sheet_area - used_area) / sheet_area * 100.0) if sheet_area else 100.0,
        "min_util_pct": min(utils) if utils else 0.0,
        "avg_util_pct": sum(utils) / len(utils) if utils else 0.0,
    }


def render_svg(req: dict[str, Any], placements: list[Placement], sheet_count: int, scale: int, out_path: Path) -> None:
    stock = req["stock"][0]
    sheet_w = float(stock["width_mm"])
    sheet_h = float(stock["height_mm"])
    usable_w, usable_h = usable_size(req, scale)
    trim = req["params"]["trim_mm"]
    gap_y = 50.0
    height = sheet_count * sheet_h + max(0, sheet_count - 1) * gap_y
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{sheet_w}" height="{height}" '
        f'viewBox="0 0 {sheet_w} {height}">',
        '<rect width="100%" height="100%" fill="#f7f7f7"/>',
    ]
    for sheet_idx in range(sheet_count):
        y0 = sheet_idx * (sheet_h + gap_y)
        parts.append(
            f'<rect x="0" y="{y0}" width="{sheet_w}" height="{sheet_h}" '
            'fill="#fff" stroke="#888" stroke-width="2"/>'
        )
        parts.append(
            f'<rect x="{trim["left"]}" y="{y0 + trim["top"]}" width="{usable_w / scale}" '
            f'height="{usable_h / scale}" fill="#eef7ff" stroke="#b0c4d8" stroke-width="1"/>'
        )
        parts.append(f'<text x="10" y="{y0 + 24}" font-size="22">sheet {sheet_idx + 1}</text>')
        for placement in [p for p in placements if p.sheet_idx == sheet_idx]:
            x = float(trim["left"]) + placement.x / scale
            y = y0 + float(trim["top"]) + placement.y / scale
            w = placement.w / scale
            h = placement.h / scale
            color = colors[placement.piece.type_idx % len(colors)]
            parts.append(
                f'<rect x="{x:.3f}" y="{y:.3f}" width="{w:.3f}" height="{h:.3f}" '
                f'fill="{color}" stroke="#9a1515" stroke-width="2"/>'
            )
            parts.append(
                f'<text x="{x + 6:.3f}" y="{y + 18:.3f}" font-size="16">{placement.piece.id}</text>'
            )
    parts.append("</svg>")
    out_path.write_text("\n".join(parts), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fixture",
        type=Path,
        default=ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json",
    )
    parser.add_argument("--repeat-factor", type=int, default=1)
    parser.add_argument("--sheet-count", type=int)
    parser.add_argument("--scale", type=int, default=10)
    parser.add_argument("--time-limit-s", type=float, default=30.0)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--objective", choices=["none", "anchor"], default="anchor")
    parser.add_argument("--max-pieces", type=int, default=80)
    parser.add_argument("--force-large", action="store_true")
    parser.add_argument("--hint-svg", type=Path)
    parser.add_argument("--fix-hint", action="store_true")
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=find_artifact_root() / "ai_docs" / "tmp" / "v65_cpsat_verifier",
    )
    args = parser.parse_args()

    req = load_fixture(args.fixture, args.repeat_factor)
    pieces = expand_pieces(req, args.scale)
    usable_w, usable_h = usable_size(req, args.scale)
    gap = scaled(float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"]), args.scale)
    lower_bound = lower_bound_sheets(pieces, usable_w, usable_h)
    sheet_count = args.sheet_count or lower_bound
    if len(pieces) > args.max_pieces and not args.force_large:
        raise SystemExit(
            f"Refusing {len(pieces)} pieces with max-pieces={args.max_pieces}; "
            "use --force-large for stress tests."
        )
    hints: dict[int, HintPlacement] = {}
    if args.hint_svg:
        hints = parse_hint_svg(req, pieces, args.scale, args.hint_svg)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    summary, placements = solve_fixed_sheet_count(
        pieces=pieces,
        usable_w=usable_w,
        usable_h=usable_h,
        gap=gap,
        sheet_count=sheet_count,
        time_limit_s=args.time_limit_s,
        workers=args.workers,
        objective=args.objective,
        hints=hints,
        fix_hint=args.fix_hint,
    )
    summary.update(
        {
            "fixture": str(args.fixture),
            "repeat_factor": args.repeat_factor,
            "scale": args.scale,
            "gap_scaled": gap,
            "gap_mm": gap / args.scale,
            "usable_w_mm": usable_w / args.scale,
            "usable_h_mm": usable_h / args.scale,
            "lower_bound_sheets_by_area": lower_bound,
            "hint_svg": str(args.hint_svg) if args.hint_svg else None,
            "hint_coverage": len(hints) / len(pieces) if pieces else 0.0,
        }
    )
    if summary["feasible"]:
        summary.update(summarize_waste(placements, sheet_count, usable_w, usable_h))
        render_svg(req, placements, sheet_count, args.scale, args.out_dir / "cpsat_layout.svg")
    (args.out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    if args.repeat_factor > 1:
        (args.out_dir / f"generated_repeat_{args.repeat_factor}.json").write_text(
            json.dumps(req, indent=2),
            encoding="utf-8",
        )
    print(json.dumps(summary, indent=2))
    return 0 if summary["feasible"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
