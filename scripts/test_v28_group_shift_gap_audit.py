"""V28 research: scan saved SVG layouts for group-shift gap-collapse moves.

This script is intentionally a research/audit tool, not production geometry.
It parses saved Freecut SVGs, groups touching/near-touching part rectangles into
connected components, and looks for rigid translations that can pull a whole
component toward the main pile without collisions.  The goal is to validate the
user's "move the group, not a single part" hypothesis before changing the
optimizer pipeline.
"""

import csv
import json
import math
import os
import re
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass
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
OUT_DIR = ARTIFACT_ROOT / "best_layouts_v28_group_shift_gap_audit"
OUT_DIR.mkdir(parents=True, exist_ok=True)

SOURCE_DIRS = [
    ARTIFACT_ROOT / "best_layouts_v8_20s",
    ARTIFACT_ROOT / "best_layouts_v9b",
    ARTIFACT_ROOT / "best_layouts_v22_profile_pool_zp04_guard08_30sweep",
    ARTIFACT_ROOT / "best_layouts_v26_v25_adaptive_sweeps" / "gt5_only",
    ARTIFACT_ROOT / "best_layouts_v26_v25_adaptive_sweeps" / "z5_corner300",
]

SVG_NS = "{http://www.w3.org/2000/svg}"
PART_FILL = "#cfe8ff"
SHEET_FILL = "#f5f5f5"
REQUIRED_GAP_MM = float(os.environ.get("FREECUT_GROUP_SHIFT_REQUIRED_GAP_MM", "6.5"))
CONTACT_TOL_MM = float(os.environ.get("FREECUT_GROUP_SHIFT_CONTACT_TOL_MM", "8.0"))
MIN_SHIFT_MM = float(os.environ.get("FREECUT_GROUP_SHIFT_MIN_SHIFT_MM", "5.0"))


@dataclass(frozen=True)
class Rect:
    idx: int
    sheet_idx: int
    x: float
    y: float
    w: float
    h: float

    @property
    def right(self) -> float:
        return self.x + self.w

    @property
    def bottom(self) -> float:
        return self.y + self.h

    @property
    def area(self) -> float:
        return self.w * self.h


@dataclass(frozen=True)
class Sheet:
    idx: int
    x: float
    y: float
    w: float
    h: float
    origin_x: float
    origin_y: float


def rect_attr(node: ET.Element, name: str) -> float:
    return float(node.attrib.get(name, "0"))


def parse_svg(path: Path) -> tuple[list[Sheet], list[Rect]]:
    root = ET.fromstring(path.read_text(encoding="utf-8"))
    sheet_nodes = []
    part_nodes = []
    for node in root.iter(f"{SVG_NS}rect"):
        fill = node.attrib.get("fill", "").lower()
        if fill == SHEET_FILL:
            sheet_nodes.append(node)
        elif fill == PART_FILL:
            part_nodes.append(node)

    sheets: list[Sheet] = []
    for idx, node in enumerate(sheet_nodes, start=1):
        x = rect_attr(node, "x")
        y = rect_attr(node, "y")
        w = rect_attr(node, "width")
        h = rect_attr(node, "height")
        sheets.append(Sheet(idx, x, y, w, h, x + 10.0, y + 10.0))

    parts: list[Rect] = []
    for idx, node in enumerate(part_nodes):
        x = rect_attr(node, "x")
        y = rect_attr(node, "y")
        w = rect_attr(node, "width")
        h = rect_attr(node, "height")
        sheet = next(
            (
                item
                for item in sheets
                if item.y <= y <= item.y + item.h and item.x <= x <= item.x + item.w
            ),
            None,
        )
        if sheet is None:
            continue
        parts.append(
            Rect(
                idx=idx,
                sheet_idx=sheet.idx,
                x=x - sheet.origin_x,
                y=y - sheet.origin_y,
                w=w,
                h=h,
            )
        )
    return sheets, parts


def overlap(a1: float, a2: float, b1: float, b2: float) -> float:
    return max(0.0, min(a2, b2) - max(a1, b1))


def touching_or_near(a: Rect, b: Rect, tol: float) -> bool:
    x_overlap = overlap(a.x, a.right, b.x, b.right)
    y_overlap = overlap(a.y, a.bottom, b.y, b.bottom)
    horizontal_gap = max(0.0, max(a.x, b.x) - min(a.right, b.right))
    vertical_gap = max(0.0, max(a.y, b.y) - min(a.bottom, b.bottom))
    return (x_overlap > 1.0 and vertical_gap <= tol) or (
        y_overlap > 1.0 and horizontal_gap <= tol
    )


def components(parts: list[Rect]) -> list[list[int]]:
    by_idx = {part.idx: part for part in parts}
    unseen = set(by_idx)
    out: list[list[int]] = []
    while unseen:
        root = unseen.pop()
        stack = [root]
        comp = [root]
        while stack:
            current = by_idx[stack.pop()]
            neighbors = [
                other_idx
                for other_idx in list(unseen)
                if touching_or_near(current, by_idx[other_idx], CONTACT_TOL_MM)
            ]
            for other_idx in neighbors:
                unseen.remove(other_idx)
                stack.append(other_idx)
                comp.append(other_idx)
        out.append(sorted(comp))
    return out


def nearest_neighbor_distances(parts: list[Rect]) -> dict[int, float]:
    distances: dict[int, float] = {}
    for part in parts:
        best = math.inf
        for other in parts:
            if other.idx == part.idx:
                continue
            dx = max(0.0, max(part.x, other.x) - min(part.right, other.right))
            dy = max(0.0, max(part.y, other.y) - min(part.bottom, other.bottom))
            best = min(best, math.hypot(dx, dy))
        distances[part.idx] = best if math.isfinite(best) else 0.0
    return distances


def anchor_group(parts: list[Rect]) -> set[int]:
    if len(parts) <= 2:
        return {part.idx for part in parts}
    distances = nearest_neighbor_distances(parts)
    ordered = sorted(distances.values())
    threshold = max(CONTACT_TOL_MM, ordered[max(0, int(len(ordered) * 0.65) - 1)])
    dense_parts = [part for part in parts if distances.get(part.idx, 0.0) <= threshold]
    comps = components(dense_parts)
    if not comps:
        return {part.idx for part in parts}
    return set(
        max(
            comps,
            key=lambda comp: (
                len(comp),
                sum(part.area for part in dense_parts if part.idx in set(comp)),
            ),
        )
    )


def bbox(parts: list[Rect], selected: set[int] | None = None) -> tuple[float, float, float, float]:
    chosen = [part for part in parts if selected is None or part.idx in selected]
    return (
        min(part.x for part in chosen),
        min(part.y for part in chosen),
        max(part.right for part in chosen),
        max(part.bottom for part in chosen),
    )


def bbox_area(box: tuple[float, float, float, float]) -> float:
    x1, y1, x2, y2 = box
    return max(0.0, x2 - x1) * max(0.0, y2 - y1)


def shifted_parts(parts: list[Rect], selected: set[int], dx: float, dy: float) -> list[Rect]:
    out = []
    for part in parts:
        if part.idx in selected:
            out.append(
                Rect(part.idx, part.sheet_idx, part.x + dx, part.y + dy, part.w, part.h)
            )
        else:
            out.append(part)
    return out


def max_shift(parts: list[Rect], selected: set[int], direction: str) -> tuple[float, bool]:
    selected_parts = [part for part in parts if part.idx in selected]
    obstacles = [part for part in parts if part.idx not in selected]
    limit = math.inf
    has_target_obstacle = False

    for moving in selected_parts:
        if direction == "left":
            limit = min(limit, moving.x)
            for obstacle in obstacles:
                if overlap(moving.y, moving.bottom, obstacle.y, obstacle.bottom) <= 1.0:
                    continue
                if obstacle.right <= moving.x:
                    has_target_obstacle = True
                    limit = min(limit, moving.x - obstacle.right - REQUIRED_GAP_MM)
        elif direction == "right":
            for obstacle in obstacles:
                if overlap(moving.y, moving.bottom, obstacle.y, obstacle.bottom) <= 1.0:
                    continue
                if obstacle.x >= moving.right:
                    has_target_obstacle = True
                    limit = min(limit, obstacle.x - moving.right - REQUIRED_GAP_MM)
        elif direction == "up":
            limit = min(limit, moving.y)
            for obstacle in obstacles:
                if overlap(moving.x, moving.right, obstacle.x, obstacle.right) <= 1.0:
                    continue
                if obstacle.bottom <= moving.y:
                    has_target_obstacle = True
                    limit = min(limit, moving.y - obstacle.bottom - REQUIRED_GAP_MM)
        elif direction == "down":
            for obstacle in obstacles:
                if overlap(moving.x, moving.right, obstacle.x, obstacle.right) <= 1.0:
                    continue
                if obstacle.y >= moving.bottom:
                    has_target_obstacle = True
                    limit = min(limit, obstacle.y - moving.bottom - REQUIRED_GAP_MM)
        else:
            raise ValueError(direction)

    if not math.isfinite(limit):
        return 0.0, has_target_obstacle
    return max(0.0, limit), has_target_obstacle


def move_vector(direction: str, shift: float) -> tuple[float, float]:
    if direction == "left":
        return -shift, 0.0
    if direction == "right":
        return shift, 0.0
    if direction == "up":
        return 0.0, -shift
    if direction == "down":
        return 0.0, shift
    raise ValueError(direction)


def centroid(parts: list[Rect]) -> tuple[float, float]:
    area = sum(part.area for part in parts)
    if area <= 0:
        return 0.0, 0.0
    return (
        sum((part.x + part.w / 2.0) * part.area for part in parts) / area,
        sum((part.y + part.h / 2.0) * part.area for part in parts) / area,
    )


def anchor_attraction_directions(
    group_parts: list[Rect], anchor_parts: list[Rect]
) -> list[str]:
    gx, gy = centroid(group_parts)
    ax, ay = centroid(anchor_parts)
    dx = ax - gx
    dy = ay - gy
    primary = "left" if dx < 0 else "right"
    secondary = "up" if dy < 0 else "down"
    if abs(dx) >= abs(dy):
        return [primary, secondary]
    return [secondary, primary]


def evaluate_svg(path: Path) -> list[dict]:
    sheets, parts = parse_svg(path)
    opportunities = []
    for sheet in sheets:
        sheet_parts = [part for part in parts if part.sheet_idx == sheet.idx]
        if len(sheet_parts) < 3:
            continue
        all_box = bbox(sheet_parts)
        all_area_before = bbox_area(all_box)
        anchor = anchor_group(sheet_parts)
        raw_groups = components(sheet_parts)
        periphery = [part for part in sheet_parts if part.idx not in anchor]
        raw_groups.extend([[part.idx] for part in periphery])
        for comp in raw_groups:
            if len(comp) == len(sheet_parts):
                continue
            selected = set(comp)
            if selected.issubset(anchor):
                continue
            comp_parts = [part for part in sheet_parts if part.idx in selected]
            anchor_parts = [part for part in sheet_parts if part.idx in anchor]
            comp_area = sum(part.area for part in comp_parts)
            comp_box = bbox(sheet_parts, selected)
            for direction in anchor_attraction_directions(comp_parts, anchor_parts):
                shift, has_target = max_shift(sheet_parts, selected, direction)
                if shift < MIN_SHIFT_MM:
                    continue
                dx, dy = move_vector(direction, shift)
                after_parts = shifted_parts(sheet_parts, selected, dx, dy)
                all_area_after = bbox_area(bbox(after_parts))
                bbox_gain = all_area_before - all_area_after
                perpendicular_span = (
                    comp_box[3] - comp_box[1]
                    if direction in ("left", "right")
                    else comp_box[2] - comp_box[0]
                )
                corridor_closed = shift * max(0.0, perpendicular_span)
                opportunities.append(
                    {
                        "source": str(path),
                        "source_name": path.name,
                        "sheet": sheet.idx,
                        "direction": direction,
                        "mode": "anchor_attraction" if not has_target else "gap_collapse",
                        "shift_mm": round(shift, 3),
                        "component_size": len(comp),
                        "component_area_mm2": round(comp_area),
                        "component_bbox": [round(value, 3) for value in comp_box],
                        "bbox_gain_mm2": round(bbox_gain),
                        "bbox_gain_pct": round(bbox_gain / all_area_before * 100.0, 3),
                        "corridor_closed_mm2": round(corridor_closed),
                        "score_mm2": round(max(bbox_gain, corridor_closed)),
                        "part_count": len(sheet_parts),
                        "anchor_size": len(anchor),
                        "selected_ids": sorted(comp),
                    }
                )
        cut_groups: list[tuple[str, set[int]]] = []
        for cut in sorted({part.x for part in sheet_parts} | {part.right for part in sheet_parts}):
            right_side = {part.idx for part in sheet_parts if part.x >= cut}
            left_side = {part.idx for part in sheet_parts if part.right <= cut}
            if 0 < len(right_side) < len(sheet_parts):
                cut_groups.append(("left", right_side))
            if 0 < len(left_side) < len(sheet_parts):
                cut_groups.append(("right", left_side))
        for cut in sorted({part.y for part in sheet_parts} | {part.bottom for part in sheet_parts}):
            bottom_side = {part.idx for part in sheet_parts if part.y >= cut}
            top_side = {part.idx for part in sheet_parts if part.bottom <= cut}
            if 0 < len(bottom_side) < len(sheet_parts):
                cut_groups.append(("up", bottom_side))
            if 0 < len(top_side) < len(sheet_parts):
                cut_groups.append(("down", top_side))

        seen_cut_groups: set[tuple[str, tuple[int, ...]]] = set()
        for direction, selected in cut_groups:
            key = (direction, tuple(sorted(selected)))
            if key in seen_cut_groups:
                continue
            seen_cut_groups.add(key)
            comp_parts = [part for part in sheet_parts if part.idx in selected]
            comp_area = sum(part.area for part in comp_parts)
            comp_box = bbox(sheet_parts, selected)
            shift, has_target = max_shift(sheet_parts, selected, direction)
            if not has_target or shift < MIN_SHIFT_MM:
                continue
            dx, dy = move_vector(direction, shift)
            after_parts = shifted_parts(sheet_parts, selected, dx, dy)
            all_area_after = bbox_area(bbox(after_parts))
            bbox_gain = all_area_before - all_area_after
            perpendicular_span = (
                comp_box[3] - comp_box[1]
                if direction in ("left", "right")
                else comp_box[2] - comp_box[0]
            )
            corridor_closed = shift * max(0.0, perpendicular_span)
            opportunities.append(
                {
                    "source": str(path),
                    "source_name": path.name,
                    "sheet": sheet.idx,
                    "direction": direction,
                    "mode": "cutline_side_group",
                    "shift_mm": round(shift, 3),
                    "component_size": len(selected),
                    "component_area_mm2": round(comp_area),
                    "component_bbox": [round(value, 3) for value in comp_box],
                    "bbox_gain_mm2": round(bbox_gain),
                    "bbox_gain_pct": round(bbox_gain / all_area_before * 100.0, 3),
                    "corridor_closed_mm2": round(corridor_closed),
                    "score_mm2": round(max(bbox_gain, corridor_closed)),
                    "part_count": len(sheet_parts),
                    "anchor_size": len(sheet_parts) - len(selected),
                    "selected_ids": sorted(selected),
                }
            )
    return opportunities


def svg_sort_key(path: Path) -> tuple[int, str]:
    match = re.search(r"rank_(\d+)", path.name)
    rank = int(match.group(1)) if match else 999
    return rank, path.name


def write_csv(path: Path, rows: list[dict]) -> None:
    if not rows:
        return
    fields = [
        "source_name",
        "sheet",
        "mode",
        "direction",
        "shift_mm",
        "component_size",
        "component_area_mm2",
        "bbox_gain_mm2",
        "bbox_gain_pct",
        "corridor_closed_mm2",
        "score_mm2",
        "part_count",
        "source",
    ]
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field) for field in fields})


def make_preview(path: Path, opportunity: dict, out_path: Path) -> None:
    _sheets, parts = parse_svg(path)
    sheet_idx = int(opportunity["sheet"])
    selected = set(int(value) for value in opportunity["selected_ids"])
    shift = float(opportunity["shift_mm"])
    dx, dy = move_vector(str(opportunity["direction"]), shift)
    sheet_parts = [part for part in parts if part.sheet_idx == sheet_idx]
    after_parts = shifted_parts(sheet_parts, selected, dx, dy)
    before_box = bbox(sheet_parts)
    width = max(1.0, before_box[2] - before_box[0])
    height = max(1.0, before_box[3] - before_box[1])
    pad = 40
    gap = 80
    canvas_w = width * 2 + gap + pad * 2
    canvas_h = height + pad * 2 + 40

    def draw_rect(part: Rect, offset_x: float, title_y: float) -> str:
        x = offset_x + (part.x - before_box[0])
        y = pad + 30 + (part.y - before_box[1])
        fill = "#8fd1a5" if part.idx in selected else "#cfe8ff"
        stroke = "#0b6b2d" if part.idx in selected else "#1f4a6d"
        return (
            f'<rect x="{x:.3f}" y="{y:.3f}" width="{part.w:.3f}" height="{part.h:.3f}" '
            f'fill="{fill}" stroke="{stroke}" stroke-width="1"/>'
        )

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {canvas_w:.3f} {canvas_h:.3f}">',
        '<rect x="0" y="0" width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{pad}" y="22" font-size="16" fill="#222">before: {path.name} sheet {sheet_idx}</text>',
        f'<text x="{pad + width + gap}" y="22" font-size="16" fill="#222">after: {opportunity["direction"]} {shift:.1f} mm</text>',
        f'<rect x="{pad}" y="{pad + 30}" width="{width:.3f}" height="{height:.3f}" fill="#f7f7f7" stroke="#999"/>',
        f'<rect x="{pad + width + gap}" y="{pad + 30}" width="{width:.3f}" height="{height:.3f}" fill="#f7f7f7" stroke="#999"/>',
    ]
    lines.extend(draw_rect(part, pad, pad) for part in sheet_parts)
    lines.extend(draw_rect(part, pad + width + gap, pad) for part in after_parts)
    lines.append("</svg>")
    out_path.write_text("\n".join(lines), encoding="utf-8")
    try:
        from PIL import Image, ImageDraw
    except Exception:
        return

    scale = min(0.22, 1800.0 / canvas_w, 1100.0 / canvas_h)
    img_w = max(1, int(canvas_w * scale))
    img_h = max(1, int(canvas_h * scale))
    image = Image.new("RGB", (img_w, img_h), "white")
    draw = ImageDraw.Draw(image)

    def sx(value: float) -> int:
        return int(round(value * scale))

    def sy(value: float) -> int:
        return int(round(value * scale))

    draw.text((sx(pad), sy(10)), f"before: {path.name} sheet {sheet_idx}", fill=(30, 30, 30))
    draw.text(
        (sx(pad + width + gap), sy(10)),
        f"after: {opportunity['direction']} {shift:.1f} mm",
        fill=(30, 30, 30),
    )
    for x0 in (pad, pad + width + gap):
        draw.rectangle(
            [sx(x0), sy(pad + 30), sx(x0 + width), sy(pad + 30 + height)],
            fill=(247, 247, 247),
            outline=(150, 150, 150),
        )

    def draw_part(part: Rect, offset_x: float) -> None:
        x = offset_x + (part.x - before_box[0])
        y = pad + 30 + (part.y - before_box[1])
        fill = (143, 209, 165) if part.idx in selected else (207, 232, 255)
        stroke = (11, 107, 45) if part.idx in selected else (31, 74, 109)
        draw.rectangle(
            [sx(x), sy(y), sx(x + part.w), sy(y + part.h)],
            fill=fill,
            outline=stroke,
        )

    for part in sheet_parts:
        draw_part(part, pad)
    for part in after_parts:
        draw_part(part, pad + width + gap)
    image.save(out_path.with_suffix(".png"))


def main() -> None:
    svg_paths = []
    for source_dir in SOURCE_DIRS:
        if source_dir.exists():
            svg_paths.extend(sorted(source_dir.glob("rank_*.svg"), key=svg_sort_key))
    all_ops: list[dict] = []
    for path in svg_paths:
        all_ops.extend(evaluate_svg(path))

    ranked = sorted(
        all_ops,
        key=lambda item: (
            -float(item["score_mm2"]),
            -float(item["bbox_gain_mm2"]),
            -float(item["shift_mm"]),
            int(item["component_size"]),
        ),
    )
    summary = {
        "source_dirs": [str(path) for path in SOURCE_DIRS],
        "svg_count": len(svg_paths),
        "opportunity_count": len(ranked),
        "min_shift_mm": MIN_SHIFT_MM,
        "required_gap_mm": REQUIRED_GAP_MM,
        "contact_tol_mm": CONTACT_TOL_MM,
        "top_opportunities": ranked[:20],
    }
    (OUT_DIR / "v28_group_shift_gap_audit_summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    write_csv(OUT_DIR / "v28_group_shift_gap_audit_opportunities.csv", ranked)
    for idx, item in enumerate(ranked[:5], start=1):
        make_preview(
            Path(item["source"]),
            item,
            OUT_DIR / f"preview_{idx:02d}_{Path(item['source']).stem}_sheet{item['sheet']}.svg",
        )
    multi_ranked = [item for item in ranked if int(item["component_size"]) > 1]
    for idx, item in enumerate(multi_ranked[:5], start=1):
        make_preview(
            Path(item["source"]),
            item,
            OUT_DIR / f"preview_multi_{idx:02d}_{Path(item['source']).stem}_sheet{item['sheet']}.svg",
        )

    print("V28 group-shift gap audit")
    print(f"svg_count={len(svg_paths)}")
    print(f"opportunity_count={len(ranked)}")
    print(f"multi_part_opportunity_count={len(multi_ranked)}")
    for idx, item in enumerate(ranked[:10], start=1):
        print(
            f"{idx:02d}. {item['source_name']} sheet={item['sheet']} "
            f"dir={item['direction']} shift={item['shift_mm']}mm "
            f"component={item['component_size']} gain={item['bbox_gain_mm2']}mm2"
        )
    print(f"summary={OUT_DIR / 'v28_group_shift_gap_audit_summary.json'}")


if __name__ == "__main__":
    main()
