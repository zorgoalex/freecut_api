#!/usr/bin/env python3
"""V67: practical ladder benchmark from small jobs to ~50 sheets.

The benchmark generates deterministic cases from one base fixture with requested
area lower bounds, then compares:
- Freecut `engine=heuristic`
- an internal Python MaxRects-style constructive portfolio
- external PackingSolver, when the local CLI is available

All generated inputs and artifacts are written under ai_docs/tmp.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import random
import re
import subprocess
import sys
import time
import urllib.request
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


def default_solver_exe() -> Path:
    return artifact_path(
        "external",
        "packingsolver",
        "build_v63_vs",
        "src",
        "rectangleguillotine",
        "Release",
        "packingsolver_rectangleguillotine.exe",
    )


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


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def usable_size(req: dict[str, Any]) -> tuple[float, float]:
    stock = req["stock"][0]
    trim = req["params"]["trim_mm"]
    return (
        float(stock["width_mm"]) - float(trim["left"]) - float(trim["right"]),
        float(stock["height_mm"]) - float(trim["top"]) - float(trim["bottom"]),
    )


def expand_pieces(req: dict[str, Any]) -> list[Piece]:
    out: list[Piece] = []
    for type_idx, item in enumerate(req["items"]):
        for copy_idx in range(int(item["qty"])):
            out.append(
                Piece(
                    id=item["id"],
                    type_idx=type_idx,
                    copy_idx=copy_idx,
                    w=float(item["width_mm"]),
                    h=float(item["height_mm"]),
                    allow_rotate=item.get("rotation", "allow_90") == "allow_90",
                )
            )
    return out


def lower_bound_sheets(req: dict[str, Any]) -> int:
    usable_w, usable_h = usable_size(req)
    total_area = sum(piece.area for piece in expand_pieces(req))
    return math.ceil(total_area / (usable_w * usable_h))


def build_piece_stream(base_req: dict[str, Any], min_target: int) -> list[dict[str, Any]]:
    usable_w, usable_h = usable_size(base_req)
    sheet_area = usable_w * usable_h
    base_items = []
    for item in base_req["items"]:
        for _copy_idx in range(int(item["qty"])):
            base_items.append(json.loads(json.dumps(item)))
    cycle_area = sum(float(item["width_mm"]) * float(item["height_mm"]) for item in base_items)
    cycles = max(2, math.ceil((min_target * sheet_area) / cycle_area) + 2)
    stream: list[dict[str, Any]] = []
    for cycle_idx in range(cycles):
        for item in base_items:
            copied = json.loads(json.dumps(item))
            copied["_cycle_idx"] = cycle_idx
            stream.append(copied)
    return stream


def build_case_for_target(base_req: dict[str, Any], stream: list[dict[str, Any]], target_lb: int) -> dict[str, Any]:
    req = json.loads(json.dumps(base_req))
    usable_w, usable_h = usable_size(req)
    sheet_area = usable_w * usable_h
    selected: list[dict[str, Any]] = []
    total_area = 0.0
    for item in stream:
        selected.append(item)
        total_area += float(item["width_mm"]) * float(item["height_mm"])
        if total_area > (target_lb - 1) * sheet_area:
            break

    grouped: dict[tuple[str, float, float, str, str], dict[str, Any]] = {}
    for item in selected:
        key = (
            item["id"],
            float(item["width_mm"]),
            float(item["height_mm"]),
            item.get("rotation", "allow_90"),
            item.get("pattern_direction", "none"),
        )
        if key not in grouped:
            grouped[key] = {
                "id": item["id"],
                "width_mm": float(item["width_mm"]),
                "height_mm": float(item["height_mm"]),
                "qty": 0,
                "rotation": item.get("rotation", "allow_90"),
                "pattern_direction": item.get("pattern_direction", "none"),
            }
        grouped[key]["qty"] += 1
    req["items"] = list(grouped.values())
    lb = lower_bound_sheets(req)
    req["stock"][0]["qty"] = max(int(req["stock"][0].get("qty", 0)), lb + 12)
    req["params"]["seed"] = int(req["params"].get("seed") or 12345)
    req["params"]["include_svg"] = False
    return req


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
    else:
        key = lambda p: (-abs(p.w - p.h), p.area)
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
    return [rect for i, rect in enumerate(rects) if not any(i != j and contained(rect, other) for j, other in enumerate(rects))]


def orientations(piece: Piece) -> list[tuple[float, float, bool]]:
    out = [(piece.w, piece.h, False)]
    if piece.allow_rotate and abs(piece.w - piece.h) > 1e-9:
        out.append((piece.h, piece.w, True))
    return out


def score_placement(free: Rect, used_w: float, used_h: float, rule: str) -> tuple[float, ...]:
    leftover_w = free.w - used_w
    leftover_h = free.h - used_h
    if rule == "bssf":
        return (min(leftover_w, leftover_h), max(leftover_w, leftover_h), free.area - used_w * used_h)
    if rule == "baf":
        return (free.area - used_w * used_h, min(leftover_w, leftover_h), max(leftover_w, leftover_h))
    if rule == "bl":
        return (free.y, free.x, min(leftover_w, leftover_h), free.area - used_w * used_h)
    contact = 0.0
    if abs(free.x) < 1e-9:
        contact += used_h
    if abs(free.y) < 1e-9:
        contact += used_w
    return (-contact, free.area - used_w * used_h, min(leftover_w, leftover_h))


def find_best_slot(sheets: list[Sheet], piece: Piece, usable_w: float, usable_h: float, gap: float, rule: str):
    best = None
    for sheet_idx, sheet in enumerate(sheets):
        for free in sheet.free:
            for actual_w, actual_h, rotated in orientations(piece):
                used_w = actual_w + gap
                used_h = actual_h + gap
                if used_w > free.w + 1e-9 or used_h > free.h + 1e-9:
                    continue
                if free.x + actual_w > usable_w + 1e-9 or free.y + actual_h > usable_h + 1e-9:
                    continue
                score = (sheet_idx,) + score_placement(free, used_w, used_h, rule)
                current = (sheet_idx, free, actual_w, actual_h, rotated, score)
                if best is None or score < best[-1]:
                    best = current
    return best


def pack_order(ordered: list[Piece], usable_w: float, usable_h: float, gap: float, rule: str) -> tuple[list[Sheet], list[Piece]]:
    sheets: list[Sheet] = []
    unplaced: list[Piece] = []
    sheet_free = Rect(0.0, 0.0, usable_w + gap, usable_h + gap)
    for piece in ordered:
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
        used = Rect(placement.x, placement.y, placement.w + gap, placement.h + gap)
        new_free: list[Rect] = []
        for free_rect in sheets[sheet_idx].free:
            new_free.extend(split_free_rect(free_rect, used))
        sheets[sheet_idx].free = prune_free_rects(new_free)
        sheets[sheet_idx].placements.append(placement)
    return sheets, unplaced


def noisy_area_order(pieces: list[Piece], rng: random.Random) -> list[Piece]:
    scored = []
    for piece in pieces:
        jitter = rng.uniform(0.82, 1.18)
        aspect = max(piece.w, piece.h) / max(1.0, min(piece.w, piece.h))
        scored.append((piece.area * jitter, max(piece.w, piece.h), -aspect, rng.random(), piece))
    return [piece for *_prefix, piece in sorted(scored, reverse=True)]


def summarize_sheets(sheets: list[Sheet], unplaced: list[Piece], pieces: list[Piece], usable_w: float, usable_h: float) -> dict[str, Any]:
    used_area = sum(piece.area for piece in pieces) - sum(piece.area for piece in unplaced)
    sheet_area = usable_w * usable_h * len(sheets)
    return {
        "sheets": len(sheets),
        "unplaced": len(unplaced),
        "waste_percent": ((sheet_area - used_area) / sheet_area * 100.0) if sheet_area else 100.0,
    }


def run_constructive(req: dict[str, Any], random_restarts: int, seed: int) -> tuple[dict[str, Any], list[Sheet]]:
    pieces = expand_pieces(req)
    usable_w, usable_h = usable_size(req)
    gap = float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"])
    candidates: list[tuple[dict[str, Any], list[Sheet]]] = []
    started = time.perf_counter()
    for sort_key in ["area", "maxside", "height", "width", "perimeter", "squareness"]:
        for rule in ["bssf", "baf", "bl", "contact"]:
            sheets, unplaced = pack_order(sort_pieces(pieces, sort_key), usable_w, usable_h, gap, rule)
            summary = summarize_sheets(sheets, unplaced, pieces, usable_w, usable_h)
            summary.update({"sort_key": sort_key, "rule": rule})
            candidates.append((summary, sheets))
    rng = random.Random(seed)
    for restart_idx in range(random_restarts):
        order = noisy_area_order(pieces, rng)
        for rule in ["bssf", "baf", "bl", "contact"]:
            sheets, unplaced = pack_order(order, usable_w, usable_h, gap, rule)
            summary = summarize_sheets(sheets, unplaced, pieces, usable_w, usable_h)
            summary.update({"sort_key": f"random_area_{restart_idx:03d}", "rule": rule})
            candidates.append((summary, sheets))
    wall_ms = (time.perf_counter() - started) * 1000.0
    candidates.sort(key=lambda pair: (pair[0]["unplaced"], pair[0]["sheets"], pair[0]["waste_percent"]))
    best, best_sheets = candidates[0]
    best = dict(best)
    best["wall_ms"] = wall_ms
    best["candidate_count"] = len(candidates)
    best["random_restarts"] = random_restarts
    return best, best_sheets


def render_constructive_svg(req: dict[str, Any], sheets: list[Sheet], out_path: Path) -> None:
    stock = req["stock"][0]
    sheet_w = float(stock["width_mm"])
    sheet_h = float(stock["height_mm"])
    usable_w, usable_h = usable_size(req)
    trim = req["params"]["trim_mm"]
    gap_y = 50.0
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    height = len(sheets) * sheet_h + max(0, len(sheets) - 1) * gap_y
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{sheet_w}" height="{height}" viewBox="0 0 {sheet_w} {height}">',
        '<rect width="100%" height="100%" fill="#f7f7f7"/>',
    ]
    for sheet_idx, sheet in enumerate(sheets):
        y0 = sheet_idx * (sheet_h + gap_y)
        parts.append(f'<rect x="0" y="{y0}" width="{sheet_w}" height="{sheet_h}" fill="#fff" stroke="#888" stroke-width="2"/>')
        parts.append(f'<rect x="{trim["left"]}" y="{y0 + trim["top"]}" width="{usable_w}" height="{usable_h}" fill="#eef7ff" stroke="#b0c4d8" stroke-width="1"/>')
        parts.append(f'<text x="10" y="{y0 + 24}" font-size="22">sheet {sheet_idx + 1}</text>')
        for placement in sheet.placements:
            x = float(trim["left"]) + placement.x
            y = y0 + float(trim["top"]) + placement.y
            color = colors[placement.piece.type_idx % len(colors)]
            parts.append(f'<rect x="{x:.3f}" y="{y:.3f}" width="{placement.w:.3f}" height="{placement.h:.3f}" fill="{color}" stroke="#9a1515" stroke-width="2"/>')
            parts.append(f'<text x="{x + 6:.3f}" y="{y + 18:.3f}" font-size="16">{placement.piece.id}</text>')
    parts.append("</svg>")
    out_path.write_text("\n".join(parts), encoding="utf-8")


def wait_ready(port: int, timeout_s: float) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health/ready", timeout=1.0) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(0.25)
    raise RuntimeError(f"Freecut service did not become ready on port {port}")


def post_json(url: str, payload: dict[str, Any], timeout_s: float) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=timeout_s) as response:
        return json.loads(response.read().decode("utf-8"))


def start_freecut(port: int) -> subprocess.Popen:
    env = os.environ.copy()
    env["PORT"] = str(port)
    return subprocess.Popen(
        ["cargo", "run", "--quiet"],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def run_freecut_heuristic(req: dict[str, Any], port: int) -> dict[str, Any]:
    payload = json.loads(json.dumps(req))
    payload["params"]["engine"] = "heuristic"
    payload["params"]["retry_strategy"] = "disabled"
    payload["params"]["time_limit_ms"] = 100
    payload["params"]["restarts"] = 1
    payload["params"]["include_svg"] = False
    started = time.perf_counter()
    response = post_json(f"http://127.0.0.1:{port}/v1/optimize", payload, 90.0)
    wall_ms = (time.perf_counter() - started) * 1000.0
    summary = response.get("summary", {})
    return {
        "sheets": summary.get("used_stock_count"),
        "unplaced": len(response.get("unplaced", [])),
        "waste_percent": summary.get("waste_percent"),
        "api_time_ms": summary.get("time_ms"),
        "wall_ms": wall_ms,
        "status": response.get("status"),
    }


def scaled(value: float, scale: int) -> int:
    return int(round(float(value) * scale))


def write_packingsolver_inputs(req: dict[str, Any], out_dir: Path, scale: int) -> None:
    with (out_dir / "items.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["ID", "WIDTH", "HEIGHT", "COPIES", "ORIENTED"])
        for idx, item in enumerate(req["items"]):
            writer.writerow([idx, scaled(item["width_mm"], scale), scaled(item["height_mm"], scale), int(item["qty"]), 1 if item.get("rotation") == "forbid" else 0])

    stock = req["stock"][0]
    trim = req["params"]["trim_mm"]
    with (out_dir / "bins.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["ID", "WIDTH", "HEIGHT", "COPIES", "LEFT_TRIM", "RIGHT_TRIM", "BOTTOM_TRIM", "TOP_TRIM", "LEFT_TRIM_TYPE", "RIGHT_TRIM_TYPE", "BOTTOM_TRIM_TYPE", "TOP_TRIM_TYPE"])
        writer.writerow([0, scaled(stock["width_mm"], scale), scaled(stock["height_mm"], scale), int(stock["qty"]), scaled(trim["left"], scale), scaled(trim["right"], scale), scaled(trim["bottom"], scale), scaled(trim["top"], scale), "hard", "hard", "hard", "hard"])


def parse_packingsolver_stdout(stdout: str) -> dict[str, Any]:
    metrics: dict[str, Any] = {}
    patterns = {
        "time_s": r"Time \(s\):\s+([0-9.]+)",
        "number_of_items": r"Number of items:\s+([0-9]+)\s*/\s*([0-9]+)",
        "sheets": r"Number of bins:\s+([0-9]+)\s*/\s*([0-9]+)",
        "waste_percent": r"Full waste:\s+[0-9.]+\s+\(([0-9.]+)%\)",
        "feasible": r"Feasible:\s+([01])",
    }
    for key, pattern in patterns.items():
        match = re.search(pattern, stdout)
        if not match:
            continue
        if key == "number_of_items":
            metrics["placed_items"] = int(match.group(1))
            metrics["total_items"] = int(match.group(2))
        elif key == "sheets":
            metrics["sheets"] = int(match.group(1))
            metrics["available_sheets"] = int(match.group(2))
        elif key == "feasible":
            metrics[key] = bool(int(match.group(1)))
        else:
            metrics[key] = float(match.group(1))
    return metrics


def render_packingsolver_svg(req: dict[str, Any], out_dir: Path, scale: int) -> None:
    certificate_path = out_dir / "packingsolver_certificate.csv"
    if not certificate_path.exists():
        return
    item_names = [item["id"] for item in req["items"]]
    stock = req["stock"][0]
    sheet_w = float(stock["width_mm"])
    sheet_h = float(stock["height_mm"])
    rows: list[dict[str, Any]] = []
    with certificate_path.open("r", encoding="utf-8", newline="") as handle:
        for row in csv.DictReader(handle):
            try:
                type_id = int(row["TYPE"])
            except (KeyError, ValueError):
                continue
            if type_id < 0:
                continue
            rows.append(
                {
                    "plate": int(row["PLATE_ID"]),
                    "type_id": type_id,
                    "x": float(row["X"]) / scale,
                    "y": float(row["Y"]) / scale,
                    "w": float(row["WIDTH"]) / scale,
                    "h": float(row["HEIGHT"]) / scale,
                }
            )
    if not rows:
        return
    plates = sorted({row["plate"] for row in rows})
    gap_y = 50.0
    height = len(plates) * sheet_h + max(0, len(plates) - 1) * gap_y
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{sheet_w}" height="{height}" viewBox="0 0 {sheet_w} {height}">',
        '<rect width="100%" height="100%" fill="#f7f7f7"/>',
    ]
    for sheet_idx, plate in enumerate(plates):
        y0 = sheet_idx * (sheet_h + gap_y)
        parts.append(f'<rect x="0" y="{y0}" width="{sheet_w}" height="{sheet_h}" fill="#fff" stroke="#888" stroke-width="2"/>')
        parts.append(f'<text x="10" y="{y0 + 24}" font-size="22">plate {plate}</text>')
        for row in [candidate for candidate in rows if candidate["plate"] == plate]:
            color = colors[row["type_id"] % len(colors)]
            x = row["x"]
            y = y0 + row["y"]
            label = item_names[row["type_id"]] if row["type_id"] < len(item_names) else str(row["type_id"])
            parts.append(f'<rect x="{x:.3f}" y="{y:.3f}" width="{row["w"]:.3f}" height="{row["h"]:.3f}" fill="{color}" stroke="#9a1515" stroke-width="2"/>')
            parts.append(f'<text x="{x + 6:.3f}" y="{y + 18:.3f}" font-size="16">{label}</text>')
    parts.append("</svg>")
    (out_dir / "packingsolver_layout.svg").write_text("\n".join(parts), encoding="utf-8")


def run_packingsolver(req: dict[str, Any], out_dir: Path, solver_exe: Path, time_limit_s: float, scale: int) -> dict[str, Any]:
    if not solver_exe.exists():
        return {"error": f"missing solver exe: {solver_exe}"}
    write_packingsolver_inputs(req, out_dir, scale)
    certificate_path = out_dir / "packingsolver_certificate.csv"
    cut_thickness = scaled(float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"]), scale)
    cmd = [
        str(solver_exe),
        "--items",
        str(out_dir / "items.csv"),
        "--bins",
        str(out_dir / "bins.csv"),
        "--objective",
        "bin-packing",
        "--number-of-stages",
        "3",
        "--cut-type",
        "non-exact",
        "--first-stage-orientation",
        "any",
        "--cut-thickness",
        str(cut_thickness),
        "--time-limit",
        str(time_limit_s),
        "--verbosity-level",
        "1",
        "--certificate",
        str(certificate_path),
        "--use-column-generation",
        "false",
        "--use-column-generation-2",
        "false",
        "--use-sequential-single-knapsack",
        "false",
        "--use-sequential-value-correction",
        "false",
        "--use-dichotomic-search",
        "false",
        "--use-tree-search",
        "true",
    ]
    started = time.perf_counter()
    proc = subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=max(20.0, time_limit_s + 10.0))
    wall_ms = (time.perf_counter() - started) * 1000.0
    (out_dir / "packingsolver_stdout.txt").write_text(proc.stdout, encoding="utf-8")
    metrics = parse_packingsolver_stdout(proc.stdout)
    metrics.update({"returncode": proc.returncode, "wall_ms": wall_ms})
    render_packingsolver_svg(req, out_dir, scale)
    return metrics


def dynamic_restarts(piece_count: int, max_restarts: int) -> int:
    if piece_count <= 60:
        return min(max_restarts, 80)
    if piece_count <= 180:
        return min(max_restarts, 32)
    if piece_count <= 360:
        return min(max_restarts, 16)
    return min(max_restarts, 8)


def choose_best(row: dict[str, Any]) -> str:
    engines = []
    for name in ["freecut", "constructive", "packingsolver"]:
        metrics = row.get(name) or {}
        if metrics.get("sheets") is None:
            continue
        engines.append((int(metrics["sheets"]), float(metrics.get("waste_percent") or 999.0), name))
    if not engines:
        return "none"
    return sorted(engines)[0][2]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", type=Path, default=ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json")
    parser.add_argument("--out-dir", type=Path, default=artifact_path("v67_ladder_benchmark"))
    parser.add_argument("--targets", default="1,2,3,4,5,6,7,8,9,10,15,20,25,30,35,40,45,50")
    parser.add_argument("--port", type=int, default=8127)
    parser.add_argument("--solver-exe", type=Path, default=default_solver_exe())
    parser.add_argument("--packingsolver-time-limit-s", type=float, default=1.0)
    parser.add_argument("--scale", type=int, default=10)
    parser.add_argument("--max-constructive-random-restarts", type=int, default=48)
    parser.add_argument("--skip-freecut", action="store_true")
    parser.add_argument("--skip-packingsolver", action="store_true")
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    base_req = load_json(args.fixture)
    targets = [int(part.strip()) for part in args.targets.split(",") if part.strip()]
    stream = build_piece_stream(base_req, max(targets))
    rows: list[dict[str, Any]] = []

    freecut_proc = None
    if not args.skip_freecut:
        freecut_proc = start_freecut(args.port)
        wait_ready(args.port, 120.0)

    try:
        for target in targets:
            req = build_case_for_target(base_req, stream, target)
            lb = lower_bound_sheets(req)
            pieces = expand_pieces(req)
            case_dir = args.out_dir / f"target_{target:02d}_lb_{lb:02d}"
            case_dir.mkdir(parents=True, exist_ok=True)
            (case_dir / "request.json").write_text(json.dumps(req, indent=2), encoding="utf-8")

            row: dict[str, Any] = {
                "target_requested": target,
                "actual_lower_bound": lb,
                "piece_count": len(pieces),
                "total_qty": sum(int(item["qty"]) for item in req["items"]),
            }

            if not args.skip_freecut:
                row["freecut"] = run_freecut_heuristic(req, args.port)

            restarts = dynamic_restarts(len(pieces), args.max_constructive_random_restarts)
            constructive, constructive_sheets = run_constructive(req, restarts, int(req["params"].get("seed") or 12345) + target)
            row["constructive"] = constructive
            render_constructive_svg(req, constructive_sheets, case_dir / "constructive_best.svg")

            if not args.skip_packingsolver:
                row["packingsolver"] = run_packingsolver(req, case_dir, args.solver_exe, args.packingsolver_time_limit_s, args.scale)

            row["best_engine"] = choose_best(row)
            rows.append(row)
            print(
                f"target={target} lb={lb} pieces={len(pieces)} "
                f"freecut={row.get('freecut', {}).get('sheets')} "
                f"constructive={row.get('constructive', {}).get('sheets')} "
                f"packingsolver={row.get('packingsolver', {}).get('sheets')} "
                f"best={row['best_engine']}"
            )
    finally:
        if freecut_proc is not None:
            freecut_proc.terminate()
            try:
                freecut_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                freecut_proc.kill()
                freecut_proc.wait(timeout=5)

    summary = {"fixture": str(args.fixture), "targets": targets, "rows": rows}
    (args.out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    with (args.out_dir / "summary.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["target", "lower_bound", "pieces", "freecut_sheets", "constructive_sheets", "packingsolver_sheets", "best_engine", "freecut_ms", "constructive_ms", "packingsolver_ms"])
        for row in rows:
            writer.writerow([
                row["target_requested"],
                row["actual_lower_bound"],
                row["piece_count"],
                (row.get("freecut") or {}).get("sheets"),
                (row.get("constructive") or {}).get("sheets"),
                (row.get("packingsolver") or {}).get("sheets"),
                row["best_engine"],
                (row.get("freecut") or {}).get("wall_ms"),
                (row.get("constructive") or {}).get("wall_ms"),
                (row.get("packingsolver") or {}).get("wall_ms"),
            ])
    return 0


if __name__ == "__main__":
    sys.exit(main())
