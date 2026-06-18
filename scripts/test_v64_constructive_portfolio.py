#!/usr/bin/env python3
"""V64: independent constructive rectangle-packing portfolio benchmark.

This research script does not call cut-optimizer-2d. It implements a small
MaxRects-style constructive portfolio with multiple sort orders and placement
rules, then compares sheet count/waste on Freecut JSON fixtures.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import time
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
    type_idx: int
    copy_idx: int
    w: float
    h: float
    allow_rotate: bool

    @property
    def area(self) -> float:
        return self.w * self.h


@dataclass
class Rect:
    x: float
    y: float
    w: float
    h: float

    @property
    def area(self) -> float:
        return self.w * self.h

    @property
    def r(self) -> float:
        return self.x + self.w

    @property
    def b(self) -> float:
        return self.y + self.h


@dataclass
class Placement:
    piece: Piece
    sheet_idx: int
    x: float
    y: float
    w: float
    h: float
    rotated: bool


@dataclass
class Sheet:
    free: list[Rect]
    placements: list[Placement]


def load_fixture(path: Path, repeat_factor: int) -> dict[str, Any]:
    req = json.loads(path.read_text(encoding="utf-8"))
    if repeat_factor > 1:
        for item in req["items"]:
            item["qty"] = int(item["qty"]) * repeat_factor
    return req


def expand_pieces(req: dict[str, Any]) -> list[Piece]:
    pieces: list[Piece] = []
    for type_idx, item in enumerate(req["items"]):
        for copy_idx in range(int(item["qty"])):
            pieces.append(
                Piece(
                    id=item["id"],
                    type_idx=type_idx,
                    copy_idx=copy_idx,
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


def sort_pieces(pieces: list[Piece], sort_key: str) -> list[Piece]:
    if sort_key == "area":
        key = lambda p: (p.area, max(p.w, p.h), min(p.w, p.h))
    elif sort_key == "maxside":
        key = lambda p: (max(p.w, p.h), p.area, min(p.w, p.h))
    elif sort_key == "height":
        key = lambda p: (p.h, p.w, p.area)
    elif sort_key == "width":
        key = lambda p: (p.w, p.h, p.area)
    elif sort_key == "perimeter":
        key = lambda p: (p.w + p.h, p.area, max(p.w, p.h))
    elif sort_key == "squareness":
        key = lambda p: (-abs(p.w - p.h), p.area)
    else:
        raise ValueError(f"unknown sort key: {sort_key}")
    return sorted(pieces, key=key, reverse=True)


def contained(a: Rect, b: Rect, eps: float = 1e-9) -> bool:
    return a.x + eps >= b.x and a.y + eps >= b.y and a.r <= b.r + eps and a.b <= b.b + eps


def intersects(a: Rect, b: Rect, eps: float = 1e-9) -> bool:
    return not (a.r <= b.x + eps or b.r <= a.x + eps or a.b <= b.y + eps or b.b <= a.y + eps)


def split_free_rect(free: Rect, used: Rect) -> list[Rect]:
    if not intersects(free, used):
        return [free]
    out: list[Rect] = []
    if used.x > free.x:
        out.append(Rect(free.x, free.y, used.x - free.x, free.h))
    if used.r < free.r:
        out.append(Rect(used.r, free.y, free.r - used.r, free.h))
    if used.y > free.y:
        out.append(Rect(free.x, free.y, free.w, used.y - free.y))
    if used.b < free.b:
        out.append(Rect(free.x, used.b, free.w, free.b - used.b))
    return [r for r in out if r.w > 1e-9 and r.h > 1e-9]


def prune_free_rects(rects: list[Rect]) -> list[Rect]:
    pruned: list[Rect] = []
    for i, rect in enumerate(rects):
        if any(i != j and contained(rect, other) for j, other in enumerate(rects)):
            continue
        pruned.append(rect)
    return pruned


def score_placement(free: Rect, used_w: float, used_h: float, rule: str) -> tuple[float, ...]:
    leftover_w = free.w - used_w
    leftover_h = free.h - used_h
    if rule == "bssf":
        return (min(leftover_w, leftover_h), max(leftover_w, leftover_h), free.area - used_w * used_h)
    if rule == "baf":
        return (free.area - used_w * used_h, min(leftover_w, leftover_h), max(leftover_w, leftover_h))
    if rule == "bl":
        return (free.y, free.x, min(leftover_w, leftover_h), free.area - used_w * used_h)
    if rule == "contact":
        contact = 0.0
        if abs(free.x) < 1e-9:
            contact += used_h
        if abs(free.y) < 1e-9:
            contact += used_w
        return (-contact, free.area - used_w * used_h, min(leftover_w, leftover_h))
    raise ValueError(f"unknown placement rule: {rule}")


def candidate_orientations(piece: Piece) -> list[tuple[float, float, bool]]:
    out = [(piece.w, piece.h, False)]
    if piece.allow_rotate and abs(piece.w - piece.h) > 1e-9:
        out.append((piece.h, piece.w, True))
    return out


def find_best_slot(
    sheets: list[Sheet],
    piece: Piece,
    usable_w: float,
    usable_h: float,
    gap: float,
    rule: str,
) -> tuple[int, Rect, float, float, bool, tuple[float, ...]] | None:
    best = None
    for sheet_idx, sheet in enumerate(sheets):
        for free in sheet.free:
            for actual_w, actual_h, rotated in candidate_orientations(piece):
                used_w = actual_w + gap
                used_h = actual_h + gap
                if used_w > free.w + 1e-9 or used_h > free.h + 1e-9:
                    continue
                if free.x + actual_w > usable_w + 1e-9 or free.y + actual_h > usable_h + 1e-9:
                    continue
                score = (sheet_idx,) + score_placement(free, used_w, used_h, rule)
                current = (sheet_idx, free, actual_w, actual_h, rotated, score)
                if best is None or current[-1] < best[-1]:
                    best = current
    return best


def place_on_sheet(sheet: Sheet, placement: Placement, gap: float) -> None:
    used = Rect(placement.x, placement.y, placement.w + gap, placement.h + gap)
    new_free: list[Rect] = []
    for free in sheet.free:
        new_free.extend(split_free_rect(free, used))
    sheet.free = prune_free_rects(new_free)
    sheet.placements.append(placement)


def pack_strategy(
    pieces: list[Piece],
    usable_w: float,
    usable_h: float,
    gap: float,
    sort_key: str,
    rule: str,
) -> tuple[list[Sheet], list[Piece]]:
    return pack_order(sort_pieces(pieces, sort_key), usable_w, usable_h, gap, sort_key, rule)


def pack_order(
    ordered_pieces: list[Piece],
    usable_w: float,
    usable_h: float,
    gap: float,
    order_name: str,
    rule: str,
) -> tuple[list[Sheet], list[Piece]]:
    sheets: list[Sheet] = []
    unplaced: list[Piece] = []
    sheet_free = Rect(0.0, 0.0, usable_w + gap, usable_h + gap)
    for piece in ordered_pieces:
        slot = find_best_slot(sheets, piece, usable_w, usable_h, gap, rule)
        if slot is None:
            sheets.append(Sheet(free=[Rect(sheet_free.x, sheet_free.y, sheet_free.w, sheet_free.h)], placements=[]))
            slot = find_best_slot(sheets[-1:], piece, usable_w, usable_h, gap, rule)
            if slot is None:
                unplaced.append(piece)
                continue
            sheet_idx = len(sheets) - 1
            _, free, actual_w, actual_h, rotated, _score = slot
        else:
            sheet_idx, free, actual_w, actual_h, rotated, _score = slot
        placement = Placement(piece, sheet_idx, free.x, free.y, actual_w, actual_h, rotated)
        place_on_sheet(sheets[sheet_idx], placement, gap)
    return sheets, unplaced


def sheet_count_lower_bound(pieces: list[Piece], usable_w: float, usable_h: float) -> int:
    total_area = sum(p.area for p in pieces)
    return math.ceil(total_area / (usable_w * usable_h))


def summarize_solution(
    sheets: list[Sheet],
    unplaced: list[Piece],
    pieces: list[Piece],
    usable_w: float,
    usable_h: float,
    sort_key: str,
    rule: str,
    wall_ms: float,
) -> dict[str, Any]:
    used_area = sum(p.area for p in pieces) - sum(p.area for p in unplaced)
    sheet_area = usable_w * usable_h * len(sheets)
    utils = []
    for sheet in sheets:
        area = sum(p.piece.area for p in sheet.placements)
        utils.append(area / (usable_w * usable_h) * 100.0 if usable_w * usable_h else 0.0)
    return {
        "sort_key": sort_key,
        "rule": rule,
        "sheets": len(sheets),
        "unplaced": len(unplaced),
        "waste_percent": ((sheet_area - used_area) / sheet_area * 100.0) if sheet_area else 100.0,
        "min_util_pct": min(utils) if utils else 0.0,
        "avg_util_pct": sum(utils) / len(utils) if utils else 0.0,
        "wall_ms": wall_ms,
    }


def noisy_area_order(pieces: list[Piece], rng: random.Random) -> list[Piece]:
    scored = []
    for piece in pieces:
        jitter = rng.uniform(0.82, 1.18)
        aspect = max(piece.w, piece.h) / max(1.0, min(piece.w, piece.h))
        scored.append((piece.area * jitter, max(piece.w, piece.h), -aspect, rng.random(), piece))
    return [piece for *_prefix, piece in sorted(scored, reverse=True)]


def run_portfolio(req: dict[str, Any], random_restarts: int, random_seed: int) -> tuple[dict[str, Any], list[Sheet]]:
    pieces = expand_pieces(req)
    usable_w, usable_h = usable_size(req)
    gap = float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"])
    sort_keys = ["area", "maxside", "height", "width", "perimeter", "squareness"]
    rules = ["bssf", "baf", "bl", "contact"]
    results: list[tuple[dict[str, Any], list[Sheet]]] = []
    for sort_key in sort_keys:
        for rule in rules:
            start = time.perf_counter()
            sheets, unplaced = pack_strategy(pieces, usable_w, usable_h, gap, sort_key, rule)
            wall_ms = (time.perf_counter() - start) * 1000.0
            results.append((summarize_solution(sheets, unplaced, pieces, usable_w, usable_h, sort_key, rule, wall_ms), sheets))
    rng = random.Random(random_seed)
    random_rules = ["bssf", "baf", "bl", "contact"]
    for restart_idx in range(random_restarts):
        order = noisy_area_order(pieces, rng)
        order_name = f"random_area_{restart_idx:03d}"
        for rule in random_rules:
            start = time.perf_counter()
            sheets, unplaced = pack_order(order, usable_w, usable_h, gap, order_name, rule)
            wall_ms = (time.perf_counter() - start) * 1000.0
            results.append((summarize_solution(sheets, unplaced, pieces, usable_w, usable_h, order_name, rule, wall_ms), sheets))
    results.sort(
        key=lambda pair: (
            pair[0]["unplaced"],
            pair[0]["sheets"],
            pair[0]["waste_percent"],
            -pair[0]["min_util_pct"],
            pair[0]["wall_ms"],
        )
    )
    best_summary, best_sheets = results[0]
    best_summary = dict(best_summary)
    best_summary["portfolio_candidates"] = [dict(summary) for summary, _ in results]
    best_summary["lower_bound_sheets_by_area"] = sheet_count_lower_bound(pieces, usable_w, usable_h)
    return best_summary, best_sheets


def render_svg(req: dict[str, Any], sheets: list[Sheet], out_path: Path) -> None:
    stock = req["stock"][0]
    sheet_w = float(stock["width_mm"])
    sheet_h = float(stock["height_mm"])
    usable_w, usable_h = usable_size(req)
    trim = req["params"]["trim_mm"]
    gap_y = 50.0
    height = len(sheets) * sheet_h + max(0, len(sheets) - 1) * gap_y
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{sheet_w}" height="{height}" '
        f'viewBox="0 0 {sheet_w} {height}">',
        '<rect width="100%" height="100%" fill="#f7f7f7"/>',
    ]
    for sheet_idx, sheet in enumerate(sheets):
        y0 = sheet_idx * (sheet_h + gap_y)
        parts.append(
            f'<rect x="0" y="{y0}" width="{sheet_w}" height="{sheet_h}" '
            'fill="#fff" stroke="#888" stroke-width="2"/>'
        )
        parts.append(
            f'<rect x="{trim["left"]}" y="{y0 + trim["top"]}" width="{usable_w}" '
            f'height="{usable_h}" fill="#eef7ff" stroke="#b0c4d8" stroke-width="1"/>'
        )
        parts.append(f'<text x="10" y="{y0 + 24}" font-size="22">sheet {sheet_idx + 1}</text>')
        for p in sheet.placements:
            x = float(trim["left"]) + p.x
            y = y0 + float(trim["top"]) + p.y
            color = colors[p.piece.type_idx % len(colors)]
            parts.append(
                f'<rect x="{x:.3f}" y="{y:.3f}" width="{p.w:.3f}" height="{p.h:.3f}" '
                f'fill="{color}" stroke="#9a1515" stroke-width="2"/>'
            )
            parts.append(
                f'<text x="{x + 6:.3f}" y="{y + 18:.3f}" font-size="16">{p.piece.id}</text>'
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
    parser.add_argument("--random-restarts", type=int, default=0)
    parser.add_argument("--random-seed", type=int, default=12345)
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=ARTIFACT_ROOT / "ai_docs" / "tmp" / "v64_constructive_portfolio",
    )
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    req = load_fixture(args.fixture, args.repeat_factor)
    summary, sheets = run_portfolio(req, args.random_restarts, args.random_seed)
    summary["fixture"] = str(args.fixture)
    summary["repeat_factor"] = args.repeat_factor
    summary["random_restarts"] = args.random_restarts
    summary["random_seed"] = args.random_seed
    portfolio_candidates = summary.pop("portfolio_candidates", [])
    (args.out_dir / "candidates.json").write_text(json.dumps(portfolio_candidates, indent=2), encoding="utf-8")
    summary["candidate_count"] = len(portfolio_candidates)
    summary["top_candidates"] = portfolio_candidates[:20]
    (args.out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    if args.repeat_factor > 1:
        (args.out_dir / f"generated_repeat_{args.repeat_factor}.json").write_text(
            json.dumps(req, indent=2),
            encoding="utf-8",
        )
    render_svg(req, sheets, args.out_dir / "best_layout.svg")
    console_summary = dict(summary)
    console_summary["top_candidates"] = portfolio_candidates[:5]
    print(json.dumps(console_summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
