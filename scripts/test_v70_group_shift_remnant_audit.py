#!/usr/bin/env python3
"""V70: paired group_shift remnant-quality audit.

This script calls `/v1/optimize` with `group_shift.debug_artifacts=true`, parses
the before/after SVG layouts, and computes visual free-space metrics that should
match the user-visible goal:

- less internal free space inside the part cluster;
- larger share of free area connected to the sheet boundary;
- fewer secondary free-space fragments;
- denser placement bounding boxes after accepted group/single/chain shifts.

Artifacts and raw results are written under ai_docs/tmp.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import re
import subprocess
import sys
import time
import urllib.request
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def find_artifact_root() -> Path:
    current = Path(__file__).resolve()
    for parent in current.parents:
        if parent.name == "freecut_repo" and (parent / "ai_docs" / "tmp").exists():
            return parent
    return ROOT


ARTIFACT_ROOT = find_artifact_root()


def artifact_path(*parts: str) -> Path:
    return ARTIFACT_ROOT / "ai_docs" / "tmp" / Path(*parts)


@dataclass(frozen=True)
class Rect:
    x: float
    y: float
    w: float
    h: float

    @property
    def r(self) -> float:
        return self.x + self.w

    @property
    def b(self) -> float:
        return self.y + self.h

    @property
    def area(self) -> float:
        return max(0.0, self.w) * max(0.0, self.h)


@dataclass(frozen=True)
class SheetLayout:
    sheet_idx: int
    usable_w: float
    usable_h: float
    placements: list[Rect]


CONTACT_SCORE_WEIGHT = 0.25
EPS = 1e-6


def load_request(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def attrs_from_tag(tag: str) -> dict[str, str]:
    return dict(re.findall(r'([a-zA-Z_:.-]+)="([^"]*)"', tag))


def rects_from_svg(svg: str) -> list[tuple[dict[str, str], Rect]]:
    out: list[tuple[dict[str, str], Rect]] = []
    for match in re.finditer(r"<rect\s+([^>]*)/?>", svg):
        attrs = attrs_from_tag(match.group(1))
        try:
            rect = Rect(
                x=float(attrs.get("x", "0")),
                y=float(attrs.get("y", "0")),
                w=float(attrs["width"]),
                h=float(attrs["height"]),
            )
        except (KeyError, ValueError):
            continue
        out.append((attrs, rect))
    return out


def parse_layout_svg(svg: str, req: dict[str, Any]) -> list[SheetLayout]:
    stock = req["stock"][0]
    trim = req["params"]["trim_mm"]
    stock_h = float(stock["height_mm"])
    usable_w = float(stock["width_mm"]) - float(trim["left"]) - float(trim["right"])
    usable_h = stock_h - float(trim["top"]) - float(trim["bottom"])

    sheet_rects: list[Rect] = []
    placements: list[Rect] = []
    for attrs, rect in rects_from_svg(svg):
        fill = attrs.get("fill", "").lower()
        stroke = attrs.get("stroke", "").lower()
        if fill == "#f5f5f5" and stroke == "#333":
            sheet_rects.append(rect)
        elif fill == "#cfe8ff":
            placements.append(rect)

    sheet_rects.sort(key=lambda rect: rect.y)
    layouts: list[SheetLayout] = []
    for sheet_idx, sheet in enumerate(sheet_rects):
        y_offset = sheet.y + float(trim["top"])
        local_placements = [
            Rect(p.x, p.y - y_offset, p.w, p.h)
            for p in placements
            if p.y >= y_offset - 1e-6 and p.y < y_offset + usable_h + 1e-6
        ]
        layouts.append(SheetLayout(sheet_idx, usable_w, usable_h, local_placements))
    return layouts


def bbox(rects: list[Rect]) -> Rect | None:
    if not rects:
        return None
    min_x = min(rect.x for rect in rects)
    min_y = min(rect.y for rect in rects)
    max_x = max(rect.r for rect in rects)
    max_y = max(rect.b for rect in rects)
    return Rect(min_x, min_y, max_x - min_x, max_y - min_y)


def point_in_rect(x: float, y: float, rect: Rect) -> bool:
    return rect.x <= x < rect.r and rect.y <= y < rect.b


def axis_gap(a_min: float, a_max: float, b_min: float, b_max: float) -> float:
    if a_max < b_min:
        return b_min - a_max
    if b_max < a_min:
        return a_min - b_max
    return 0.0


def overlap_length(a_min: float, a_max: float, b_min: float, b_max: float) -> float:
    return max(0.0, min(a_max, b_max) - max(a_min, b_min))


def placement_contact_mm(placements: list[Rect], gap_mm: float) -> float:
    contact_gap = gap_mm + EPS
    score = 0.0
    for idx, a in enumerate(placements):
        for b in placements[idx + 1 :]:
            horizontal_gap = axis_gap(a.x, a.r, b.x, b.r)
            vertical_gap = axis_gap(a.y, a.b, b.y, b.b)
            if horizontal_gap <= contact_gap and vertical_gap <= EPS:
                score += overlap_length(a.y, a.b, b.y, b.b)
            if vertical_gap <= contact_gap and horizontal_gap <= EPS:
                score += overlap_length(a.x, a.r, b.x, b.r)
    return score


def placement_perimeter_mm(placements: list[Rect]) -> float:
    return sum(2.0 * (rect.w + rect.h) for rect in placements)


def mark_occupied(layout: SheetLayout, step_mm: float) -> list[list[bool]]:
    cols = max(1, math.ceil(layout.usable_w / step_mm))
    rows = max(1, math.ceil(layout.usable_h / step_mm))
    occupied = [[False for _ in range(cols)] for _ in range(rows)]
    for row in range(rows):
        y = min(layout.usable_h - 1e-6, (row + 0.5) * step_mm)
        for col in range(cols):
            x = min(layout.usable_w - 1e-6, (col + 0.5) * step_mm)
            occupied[row][col] = any(point_in_rect(x, y, rect) for rect in layout.placements)
    return occupied


def flood_free_components(occupied: list[list[bool]]) -> list[dict[str, Any]]:
    rows = len(occupied)
    cols = len(occupied[0]) if rows else 0
    seen = [[False for _ in range(cols)] for _ in range(rows)]
    components: list[dict[str, Any]] = []
    for row in range(rows):
        for col in range(cols):
            if occupied[row][col] or seen[row][col]:
                continue
            queue: deque[tuple[int, int]] = deque([(row, col)])
            seen[row][col] = True
            cells = 0
            boundary_cells = 0
            corner_connected = False
            min_row = row
            max_row = row
            min_col = col
            max_col = col
            while queue:
                current_row, current_col = queue.popleft()
                cells += 1
                min_row = min(min_row, current_row)
                max_row = max(max_row, current_row)
                min_col = min(min_col, current_col)
                max_col = max(max_col, current_col)
                on_boundary = (
                    current_row == 0
                    or current_col == 0
                    or current_row == rows - 1
                    or current_col == cols - 1
                )
                if on_boundary:
                    boundary_cells += 1
                if (current_row, current_col) in {
                    (0, 0),
                    (0, cols - 1),
                    (rows - 1, 0),
                    (rows - 1, cols - 1),
                }:
                    corner_connected = True
                for next_row, next_col in (
                    (current_row - 1, current_col),
                    (current_row + 1, current_col),
                    (current_row, current_col - 1),
                    (current_row, current_col + 1),
                ):
                    if next_row < 0 or next_col < 0 or next_row >= rows or next_col >= cols:
                        continue
                    if seen[next_row][next_col] or occupied[next_row][next_col]:
                        continue
                    seen[next_row][next_col] = True
                    queue.append((next_row, next_col))
            components.append(
                {
                    "cells": cells,
                    "boundary_cells": boundary_cells,
                    "boundary_connected": boundary_cells > 0,
                    "corner_connected": corner_connected,
                    "bbox_cells": (max_row - min_row + 1) * (max_col - min_col + 1),
                }
            )
    components.sort(key=lambda item: item["cells"], reverse=True)
    return components


def sheet_metrics(layout: SheetLayout, step_mm: float, gap_mm: float) -> dict[str, Any]:
    occupied = mark_occupied(layout, step_mm)
    rows = len(occupied)
    cols = len(occupied[0]) if rows else 0
    total_cells = rows * cols
    occupied_cells = sum(1 for row in occupied for cell in row if cell)
    free_cells = total_cells - occupied_cells
    components = flood_free_components(occupied)
    largest = components[0] if components else {"cells": 0, "boundary_cells": 0}
    boundary_components = [component for component in components if component["boundary_connected"]]
    largest_boundary = max((component["cells"] for component in boundary_components), default=0)
    internal_free = sum(component["cells"] for component in components if not component["boundary_connected"])
    secondary_free = max(0, free_cells - largest["cells"])

    placement_bbox = bbox(layout.placements)
    bbox_cells = 0
    bbox_free_cells = 0
    if placement_bbox is not None:
        for row in range(rows):
            y = min(layout.usable_h - 1e-6, (row + 0.5) * step_mm)
            for col in range(cols):
                x = min(layout.usable_w - 1e-6, (col + 0.5) * step_mm)
                if point_in_rect(x, y, placement_bbox):
                    bbox_cells += 1
                    if not occupied[row][col]:
                        bbox_free_cells += 1

    component_count = len(components)
    largest_boundary_ratio = largest_boundary / free_cells if free_cells else 1.0
    internal_free_ratio = internal_free / free_cells if free_cells else 0.0
    secondary_free_ratio = secondary_free / free_cells if free_cells else 0.0
    bbox_void_ratio = bbox_free_cells / bbox_cells if bbox_cells else 0.0
    cluster_density = 1.0 - bbox_void_ratio if bbox_cells else 1.0
    topology_score = (
        largest_boundary_ratio
        - internal_free_ratio
        - 0.35 * secondary_free_ratio
        - 0.25 * bbox_void_ratio
        - 0.02 * max(0, component_count - 1)
    )
    part_contact_mm = placement_contact_mm(layout.placements, gap_mm)
    part_perimeter_mm = placement_perimeter_mm(layout.placements)
    part_contact_ratio = part_contact_mm / part_perimeter_mm if part_perimeter_mm else 0.0
    remnant_score = topology_score + CONTACT_SCORE_WEIGHT * part_contact_ratio

    return {
        "component_count": component_count,
        "free_area_mm2": free_cells * step_mm * step_mm,
        "largest_free_area_mm2": largest["cells"] * step_mm * step_mm,
        "largest_boundary_area_mm2": largest_boundary * step_mm * step_mm,
        "largest_boundary_ratio": largest_boundary_ratio,
        "internal_free_area_mm2": internal_free * step_mm * step_mm,
        "internal_free_ratio": internal_free_ratio,
        "secondary_free_area_mm2": secondary_free * step_mm * step_mm,
        "secondary_free_ratio": secondary_free_ratio,
        "bbox_void_area_mm2": bbox_free_cells * step_mm * step_mm,
        "bbox_void_ratio": bbox_void_ratio,
        "cluster_density": cluster_density,
        "boundary_contact_mm": largest["boundary_cells"] * step_mm,
        "part_contact_mm": part_contact_mm,
        "placement_perimeter_mm": part_perimeter_mm,
        "part_contact_ratio": part_contact_ratio,
        "topology_score": topology_score,
        "remnant_score": remnant_score,
    }


def aggregate_metrics(layouts: list[SheetLayout], step_mm: float, gap_mm: float) -> dict[str, Any]:
    sheet_rows = [sheet_metrics(layout, step_mm, gap_mm) for layout in layouts]
    if not sheet_rows:
        return {"sheets": 0, "sheet_metrics": []}
    total_free = sum(row["free_area_mm2"] for row in sheet_rows) or 1.0
    total_perimeter = sum(row["placement_perimeter_mm"] for row in sheet_rows) or 1.0
    part_contact_mm = sum(row["part_contact_mm"] for row in sheet_rows)
    part_contact_ratio = part_contact_mm / total_perimeter
    topology_score = (
        sum(row["topology_score"] * row["free_area_mm2"] for row in sheet_rows) / total_free
    )
    weighted = {
        "largest_boundary_ratio": sum(
            row["largest_boundary_ratio"] * row["free_area_mm2"] for row in sheet_rows
        )
        / total_free,
        "internal_free_ratio": sum(
            row["internal_free_ratio"] * row["free_area_mm2"] for row in sheet_rows
        )
        / total_free,
        "secondary_free_ratio": sum(
            row["secondary_free_ratio"] * row["free_area_mm2"] for row in sheet_rows
        )
        / total_free,
        "bbox_void_ratio": sum(
            row["bbox_void_ratio"] * row["free_area_mm2"] for row in sheet_rows
        )
        / total_free,
        "cluster_density": sum(
            row["cluster_density"] * row["free_area_mm2"] for row in sheet_rows
        )
        / total_free,
        "part_contact_ratio": part_contact_ratio,
        "topology_score": topology_score,
        "remnant_score": topology_score + CONTACT_SCORE_WEIGHT * part_contact_ratio,
    }
    return {
        "sheets": len(layouts),
        "component_count_total": sum(row["component_count"] for row in sheet_rows),
        "free_area_mm2": sum(row["free_area_mm2"] for row in sheet_rows),
        "internal_free_area_mm2": sum(row["internal_free_area_mm2"] for row in sheet_rows),
        "secondary_free_area_mm2": sum(row["secondary_free_area_mm2"] for row in sheet_rows),
        "bbox_void_area_mm2": sum(row["bbox_void_area_mm2"] for row in sheet_rows),
        "part_contact_mm": part_contact_mm,
        "placement_perimeter_mm": total_perimeter,
        **weighted,
        "sheet_metrics": sheet_rows,
    }


def delta_metrics(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    keys = [
        "component_count_total",
        "largest_boundary_ratio",
        "internal_free_ratio",
        "secondary_free_ratio",
        "bbox_void_ratio",
        "cluster_density",
        "part_contact_mm",
        "part_contact_ratio",
        "topology_score",
        "remnant_score",
        "internal_free_area_mm2",
        "secondary_free_area_mm2",
        "bbox_void_area_mm2",
    ]
    return {f"delta_{key}": after.get(key, 0.0) - before.get(key, 0.0) for key in keys}


def request_gap_mm(req: dict[str, Any]) -> float:
    params = req.get("params") or {}
    return float(params.get("kerf_mm", 0.0)) + float(params.get("spacing_mm", 0.0))


def wait_ready(port: int, timeout_s: float) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health/ready", timeout=1.0) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.25)
    raise RuntimeError(f"service did not become ready on port {port}")


def start_service(port: int, log_path: Path) -> tuple[subprocess.Popen, Any]:
    env = os.environ.copy()
    env["PORT"] = str(port)
    log_handle = log_path.open("w", encoding="utf-8")
    proc = subprocess.Popen(
        ["cargo", "run", "--quiet"],
        cwd=ROOT,
        env=env,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return proc, log_handle


def post_optimize(port: int, payload: dict[str, Any]) -> dict[str, Any]:
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/optimize",
        data=json.dumps(payload).encode("utf-8"),
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read().decode("utf-8"))


def configure_request(base_req: dict[str, Any], seed: int, args: argparse.Namespace) -> dict[str, Any]:
    req = json.loads(json.dumps(base_req))
    params = req["params"]
    params["seed"] = seed
    params["time_limit_ms"] = args.time_limit_ms
    params["restarts"] = args.restarts
    params["layout_mode"] = args.layout_mode
    params["include_svg"] = True
    params["retry_strategy"] = "disabled"
    params["group_shift"] = {
        "enabled": True,
        "debug_artifacts": True,
        "min_shift_mm": args.group_shift_min_shift_mm,
        "max_passes": args.group_shift_max_passes,
    }
    for optional in (
        "portfolio",
        "beam",
        "alns",
        "profile_pool",
        "partition",
        "consolidate",
        "lns",
        "cut_quality",
    ):
        params.pop(optional, None)
    return req


def write_artifacts(case_dir: Path, response: dict[str, Any], compact: dict[str, Any]) -> None:
    artifacts = response.get("artifacts") or {}
    for name, key in (
        ("before.svg", "group_shift_before_svg"),
        ("after.svg", "svg"),
        ("diff.svg", "group_shift_diff_svg"),
    ):
        svg = artifacts.get(key)
        if svg:
            (case_dir / name).write_text(svg, encoding="utf-8")
    response_copy = json.loads(json.dumps(response))
    if "artifacts" in response_copy:
        response_copy["artifacts"] = {
            key: f"<omitted {len(value)} chars>"
            for key, value in response_copy["artifacts"].items()
            if isinstance(value, str)
        }
    (case_dir / "response_compact.json").write_text(
        json.dumps(response_copy, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    (case_dir / "remnant_metrics.json").write_text(
        json.dumps(compact, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )


def run_case(seed: int, req: dict[str, Any], response: dict[str, Any], args: argparse.Namespace, out_dir: Path) -> dict[str, Any]:
    artifacts = response.get("artifacts") or {}
    before_svg = artifacts.get("group_shift_before_svg")
    after_svg = artifacts.get("svg")
    if not before_svg or not after_svg:
        raise RuntimeError("missing group_shift before/after SVG artifacts")
    gap_mm = request_gap_mm(req)
    before = aggregate_metrics(parse_layout_svg(before_svg, req), args.grid_step_mm, gap_mm)
    after = aggregate_metrics(parse_layout_svg(after_svg, req), args.grid_step_mm, gap_mm)
    delta = delta_metrics(before, after)
    summary = response.get("summary", {})
    group_shift = summary.get("group_shift") or {}
    row = {
        "seed": seed,
        "status": response.get("status"),
        "sheets": summary.get("used_stock_count"),
        "time_ms": summary.get("time_ms"),
        "group_moves": group_shift.get("moves_applied", 0),
        "group_parts_moved": group_shift.get("parts_moved", 0),
        "group_passes_run": group_shift.get("passes_run", 0),
        "group_time_ms": group_shift.get("time_ms", 0),
        "group_closed_area_mm2": group_shift.get("corridor_closed_area_mm2", 0.0),
        "group_contact_gain_mm": group_shift.get("contact_gain_mm", 0.0),
        "group_quality_guard_rejections": group_shift.get("quality_guard_rejections", 0),
        "group_anchor_perimeter_candidates": group_shift.get("anchor_perimeter_candidates", 0),
        "group_quality_score_before": group_shift.get("quality_score_before", 0.0),
        "group_quality_score_after": group_shift.get("quality_score_after", 0.0),
        "group_quality_score_delta": group_shift.get("quality_score_delta", 0.0),
        "group_topology_score_delta": group_shift.get("topology_score_delta", 0.0),
        "group_part_contact_delta_mm": group_shift.get("part_contact_delta_mm", 0.0),
        "before_topology_score": before["topology_score"],
        "after_topology_score": after["topology_score"],
        "before_part_contact_mm": before["part_contact_mm"],
        "after_part_contact_mm": after["part_contact_mm"],
        "before_part_contact_ratio": before["part_contact_ratio"],
        "after_part_contact_ratio": after["part_contact_ratio"],
        "before_remnant_score": before["remnant_score"],
        "after_remnant_score": after["remnant_score"],
        **delta,
    }
    case_dir = out_dir / f"seed_{seed:03d}_moves_{row['group_moves']}"
    case_dir.mkdir(parents=True, exist_ok=True)
    write_artifacts(case_dir, response, {"row": row, "before": before, "after": after, "delta": delta})
    return row


def parse_seed_list(raw: str, count: int) -> list[int]:
    if raw.strip():
        return [int(part.strip()) for part in raw.split(",") if part.strip()]
    return list(range(1, count + 1))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", type=Path, default=ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json")
    parser.add_argument("--out-dir", type=Path, default=artifact_path("v70_group_shift_remnant_audit"))
    parser.add_argument("--port", type=int, default=8130)
    parser.add_argument("--seeds", type=int, default=12)
    parser.add_argument("--seed-list", default="")
    parser.add_argument("--time-limit-ms", type=int, default=3000)
    parser.add_argument("--restarts", type=int, default=3)
    parser.add_argument("--layout-mode", choices=["guillotine", "nested"], default="guillotine")
    parser.add_argument("--group-shift-min-shift-mm", type=float, default=5.0)
    parser.add_argument("--group-shift-max-passes", type=int, default=8)
    parser.add_argument("--grid-step-mm", type=float, default=10.0)
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    base_req = load_request(args.fixture)
    seeds = parse_seed_list(args.seed_list, args.seeds)

    proc, service_log = start_service(args.port, args.out_dir / "service.log")
    rows: list[dict[str, Any]] = []
    started = time.perf_counter()
    try:
        wait_ready(args.port, 120.0)
        for seed in seeds:
            req = configure_request(base_req, seed, args)
            try:
                response = post_optimize(args.port, req)
                row = run_case(seed, req, response, args, args.out_dir)
            except Exception as exc:
                row = {"seed": seed, "error": str(exc)}
            rows.append(row)
            print(
                "seed={seed} sheets={sheets} moves={moves} parts={parts} "
                "score={before:.4f}->{after:.4f} d_score={delta:.4f} "
                "d_contact={d_contact:.1f} d_internal={d_internal:.4f} "
                "d_boundary={d_boundary:.4f}".format(
                    seed=seed,
                    sheets=row.get("sheets"),
                    moves=row.get("group_moves"),
                    parts=row.get("group_parts_moved"),
                    before=row.get("before_remnant_score", 0.0),
                    after=row.get("after_remnant_score", 0.0),
                    delta=row.get("delta_remnant_score", 0.0),
                    d_contact=row.get("delta_part_contact_mm", 0.0),
                    d_internal=row.get("delta_internal_free_ratio", 0.0),
                    d_boundary=row.get("delta_largest_boundary_ratio", 0.0),
                ),
                flush=True,
            )
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        service_log.close()

    moved = [row for row in rows if row.get("group_moves", 0) > 0]
    improved = [row for row in moved if row.get("delta_remnant_score", 0.0) > 1e-9]
    worsened = [row for row in moved if row.get("delta_remnant_score", 0.0) < -1e-9]
    summary = {
        "fixture": str(args.fixture),
        "layout_mode": args.layout_mode,
        "time_limit_ms": args.time_limit_ms,
        "restarts": args.restarts,
        "grid_step_mm": args.grid_step_mm,
        "group_shift_min_shift_mm": args.group_shift_min_shift_mm,
        "group_shift_max_passes": args.group_shift_max_passes,
        "elapsed_s": round(time.perf_counter() - started, 2),
        "counts": {
            "seeds": len(rows),
            "moved": len(moved),
            "remnant_score_improved": len(improved),
            "remnant_score_worsened": len(worsened),
        },
        "totals": {
            "moves": sum(row.get("group_moves", 0) for row in rows),
            "parts_moved": sum(row.get("group_parts_moved", 0) for row in rows),
            "closed_area_mm2": sum(row.get("group_closed_area_mm2", 0.0) for row in rows),
            "contact_gain_mm": sum(row.get("group_contact_gain_mm", 0.0) for row in rows),
            "quality_guard_rejections": sum(row.get("group_quality_guard_rejections", 0) for row in rows),
            "anchor_perimeter_candidates": sum(
                row.get("group_anchor_perimeter_candidates", 0) for row in rows
            ),
            "telemetry_quality_score_delta": sum(row.get("group_quality_score_delta", 0.0) for row in rows),
            "telemetry_topology_score_delta": sum(row.get("group_topology_score_delta", 0.0) for row in rows),
            "telemetry_part_contact_delta_mm": sum(row.get("group_part_contact_delta_mm", 0.0) for row in rows),
            "delta_remnant_score": sum(row.get("delta_remnant_score", 0.0) for row in rows),
            "delta_topology_score": sum(row.get("delta_topology_score", 0.0) for row in rows),
            "delta_part_contact_mm": sum(row.get("delta_part_contact_mm", 0.0) for row in rows),
            "delta_part_contact_ratio": sum(row.get("delta_part_contact_ratio", 0.0) for row in rows),
            "delta_internal_free_area_mm2": sum(row.get("delta_internal_free_area_mm2", 0.0) for row in rows),
            "delta_bbox_void_area_mm2": sum(row.get("delta_bbox_void_area_mm2", 0.0) for row in rows),
        },
        "rows": rows,
    }
    (args.out_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    with (args.out_dir / "summary.csv").open("w", encoding="utf-8", newline="") as handle:
        fieldnames = sorted({key for row in rows for key in row.keys()})
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    print(json.dumps(summary["counts"], indent=2), flush=True)
    print(f"Saved to {args.out_dir}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
