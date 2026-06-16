"""V42: offline visual/remnant metrics audit.

Reads saved JSON/SVG layouts and computes metrics intended to match visual
quality better than kerf-inflated waste-region counts.
"""

from __future__ import annotations

import csv
import json
import math
import os
import re
from collections import deque
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "ai_docs" / "tmp" / "v42_visual_remnant_metrics"
OUT_DIR.mkdir(parents=True, exist_ok=True)

CELL_MM = int(os.environ.get("FREECUT_VIS_CELL_MM", "10"))
MIN_ZONE_AREA_MM2 = float(os.environ.get("FREECUT_MIN_ZONE_AREA_MM2", "5000"))
LEAD_DROP_GUARD_PP = float(os.environ.get("FREECUT_LEAD_DROP_GUARD_PP", "0.8"))
CONTACT_GAP_MM = float(os.environ.get("FREECUT_CONTACT_GAP_MM", "7.0"))


@dataclass
class Rect:
    x: float
    y: float
    w: float
    h: float
    item_id: str = ""


@dataclass
class Sheet:
    width: float
    height: float
    placements: list[Rect]


@dataclass
class SheetMetrics:
    util_pct: float
    zones: int
    waste_area_mm2: float
    secondary_waste_area_mm2: float
    internal_void_area_mm2: float
    thin_waste_area_mm2: float
    largest_zone_area_mm2: float
    largest_boundary_zone_area_mm2: float
    largest_corner_zone_area_mm2: float
    cluster_bbox_density: float
    internal_gap_area_mm2: float
    part_contact_mm: float
    part_contact_ratio: float


@dataclass
class LayoutMetrics:
    label: str
    source: str
    sheets: int
    lead_util_pct: float
    min_util_pct: float
    total_zones: int
    extra_zones: int
    max_zones_per_sheet: int
    total_waste_area_mm2: float
    secondary_waste_area_mm2: float
    secondary_waste_ratio: float
    internal_void_area_mm2: float
    internal_void_ratio: float
    thin_waste_area_mm2: float
    thin_waste_ratio: float
    largest_boundary_zone_ratio: float
    largest_corner_zone_ratio: float
    avg_cluster_bbox_density: float
    min_cluster_bbox_density: float
    internal_gap_area_mm2: float
    part_contact_mm: float
    part_contact_ratio: float
    visual_loss: float
    per_sheet_zones: list[int]
    per_sheet_util_pct: list[float]


def read_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8-sig") as f:
        return json.load(f)


def load_json_layout(path: Path) -> list[Sheet]:
    data = read_json(path)
    response = data.get("response", data)
    sheets = []
    for sol in response.get("solutions", []):
        trim = sol.get("trim_mm", {}) or {}
        # API placements are already in usable-area coordinates.
        width = float(sol["width_mm"]) - float(trim.get("left", 0)) - float(trim.get("right", 0))
        height = float(sol["height_mm"]) - float(trim.get("top", 0)) - float(trim.get("bottom", 0))
        placements = [
            Rect(
                float(p["x_mm"]),
                float(p["y_mm"]),
                float(p["width_mm"]),
                float(p["height_mm"]),
                str(p.get("item_id", "")),
            )
            for p in sol.get("placements", [])
        ]
        sheets.append(Sheet(width, height, placements))
    return sheets


def parse_svg_attrs(tag: str) -> dict[str, str]:
    return {name: value for name, value in re.findall(r'([A-Za-z_:-]+)="([^"]*)"', tag)}


def load_svg_layout(path: Path) -> list[Sheet]:
    text = path.read_text(encoding="utf-8-sig")
    rects = []
    for match in re.finditer(r"<rect\s+[^>]+>", text):
        attrs = parse_svg_attrs(match.group(0))
        fill = attrs.get("fill", "").lower()
        try:
            rect = Rect(
                float(attrs["x"]),
                float(attrs["y"]),
                float(attrs["width"]),
                float(attrs["height"]),
                fill,
            )
        except (KeyError, ValueError):
            continue
        rects.append((fill, rect))

    raw_sheets = [rect for fill, rect in rects if fill == "#f5f5f5"]
    raw_parts = [rect for fill, rect in rects if fill == "#cfe8ff"]
    sheets: list[Sheet] = []
    for raw in raw_sheets:
        usable_x = raw.x + 10.0
        usable_y = raw.y + 10.0
        usable_w = raw.w - 20.0
        usable_h = raw.h - 20.0
        placements = []
        for part in raw_parts:
            cx = part.x + part.w / 2.0
            cy = part.y + part.h / 2.0
            if usable_x <= cx <= usable_x + usable_w and usable_y <= cy <= usable_y + usable_h:
                placements.append(Rect(part.x - usable_x, part.y - usable_y, part.w, part.h))
        sheets.append(Sheet(usable_w, usable_h, placements))
    return sheets


def load_layout(path: Path) -> list[Sheet]:
    if path.suffix.lower() == ".svg":
        return load_svg_layout(path)
    return load_json_layout(path)


def mark_occupancy(sheet: Sheet) -> list[list[bool]]:
    nx = max(1, int(math.ceil(sheet.width / CELL_MM)))
    ny = max(1, int(math.ceil(sheet.height / CELL_MM)))
    occ = [[False] * nx for _ in range(ny)]
    for rect in sheet.placements:
        x0 = max(0, int(math.floor(rect.x / CELL_MM)))
        y0 = max(0, int(math.floor(rect.y / CELL_MM)))
        x1 = min(nx - 1, int(math.ceil((rect.x + rect.w) / CELL_MM)) - 1)
        y1 = min(ny - 1, int(math.ceil((rect.y + rect.h) / CELL_MM)) - 1)
        for y in range(y0, y1 + 1):
            row = occ[y]
            for x in range(x0, x1 + 1):
                row[x] = True
    return occ


def overlap_len(a0: float, a1: float, b0: float, b1: float) -> float:
    return max(0.0, min(a1, b1) - max(a0, b0))


def part_contact_metrics(sheet: Sheet) -> tuple[float, float]:
    contact = 0.0
    rects = sheet.placements
    for i, a in enumerate(rects):
        for b in rects[i + 1 :]:
            vertical_overlap = overlap_len(a.y, a.y + a.h, b.y, b.y + b.h)
            horizontal_overlap = overlap_len(a.x, a.x + a.w, b.x, b.x + b.w)
            if vertical_overlap > 0:
                gap_ab = abs((a.x + a.w) - b.x)
                gap_ba = abs((b.x + b.w) - a.x)
                if min(gap_ab, gap_ba) <= CONTACT_GAP_MM:
                    contact += vertical_overlap
            if horizontal_overlap > 0:
                gap_ab = abs((a.y + a.h) - b.y)
                gap_ba = abs((b.y + b.h) - a.y)
                if min(gap_ab, gap_ba) <= CONTACT_GAP_MM:
                    contact += horizontal_overlap
    total_perimeter = sum(2.0 * (r.w + r.h) for r in rects)
    ratio = contact / total_perimeter if total_perimeter > 0 else 0.0
    return contact, ratio


def sheet_metrics(sheet: Sheet) -> SheetMetrics:
    used_area = sum(r.w * r.h for r in sheet.placements)
    usable_area = sheet.width * sheet.height
    util_pct = used_area / usable_area * 100.0 if usable_area > 0 else 0.0

    occ = mark_occupancy(sheet)
    ny = len(occ)
    nx = len(occ[0]) if ny else 0
    seen = [[False] * nx for _ in range(ny)]
    components = []
    for y in range(ny):
        for x in range(nx):
            if occ[y][x] or seen[y][x]:
                continue
            q = deque([(x, y)])
            seen[y][x] = True
            cells = []
            while q:
                cx, cy = q.popleft()
                cells.append((cx, cy))
                for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1)):
                    nx2, ny2 = cx + dx, cy + dy
                    if 0 <= nx2 < nx and 0 <= ny2 < ny and not occ[ny2][nx2] and not seen[ny2][nx2]:
                        seen[ny2][nx2] = True
                        q.append((nx2, ny2))
            area = len(cells) * CELL_MM * CELL_MM
            if area < MIN_ZONE_AREA_MM2:
                continue
            xs = [c[0] for c in cells]
            ys = [c[1] for c in cells]
            min_x, max_x = min(xs), max(xs)
            min_y, max_y = min(ys), max(ys)
            bbox_cells = (max_x - min_x + 1) * (max_y - min_y + 1)
            bbox_area = bbox_cells * CELL_MM * CELL_MM
            fill = area / bbox_area if bbox_area > 0 else 0.0
            bw = (max_x - min_x + 1) * CELL_MM
            bh = (max_y - min_y + 1) * CELL_MM
            aspect = max(bw / max(bh, 1e-9), bh / max(bw, 1e-9))
            touches_left = min_x == 0
            touches_right = max_x == nx - 1
            touches_top = min_y == 0
            touches_bottom = max_y == ny - 1
            touches_edge = touches_left or touches_right or touches_top or touches_bottom
            touches_corner = (
                (touches_left and touches_top)
                or (touches_right and touches_top)
                or (touches_left and touches_bottom)
                or (touches_right and touches_bottom)
            )
            components.append(
                {
                    "area": area,
                    "fill": fill,
                    "aspect": aspect,
                    "touches_edge": touches_edge,
                    "touches_corner": touches_corner,
                }
            )

    components.sort(key=lambda c: c["area"], reverse=True)
    waste_area = max(0.0, usable_area - used_area)
    secondary_area = sum(c["area"] for c in components[1:])
    internal_area = sum(c["area"] for c in components if not c["touches_edge"])
    thin_area = sum(c["area"] for c in components if c["fill"] < 0.25 or c["aspect"] > 5.0)
    largest_area = components[0]["area"] if components else 0.0
    largest_boundary = max((c["area"] for c in components if c["touches_edge"]), default=0.0)
    largest_corner = max((c["area"] for c in components if c["touches_corner"]), default=0.0)

    if sheet.placements:
        min_x = min(r.x for r in sheet.placements)
        min_y = min(r.y for r in sheet.placements)
        max_x = max(r.x + r.w for r in sheet.placements)
        max_y = max(r.y + r.h for r in sheet.placements)
        bbox_area = max(0.0, max_x - min_x) * max(0.0, max_y - min_y)
        cluster_density = used_area / bbox_area if bbox_area > 0 else 1.0
        internal_gap = max(0.0, bbox_area - used_area)
    else:
        cluster_density = 0.0
        internal_gap = 0.0
    part_contact, part_contact_ratio = part_contact_metrics(sheet)

    return SheetMetrics(
        util_pct=util_pct,
        zones=len(components),
        waste_area_mm2=waste_area,
        secondary_waste_area_mm2=secondary_area,
        internal_void_area_mm2=internal_area,
        thin_waste_area_mm2=thin_area,
        largest_zone_area_mm2=largest_area,
        largest_boundary_zone_area_mm2=largest_boundary,
        largest_corner_zone_area_mm2=largest_corner,
        cluster_bbox_density=cluster_density,
        internal_gap_area_mm2=internal_gap,
        part_contact_mm=part_contact,
        part_contact_ratio=part_contact_ratio,
    )


def layout_metrics(label: str, source: Path, sheets: list[Sheet]) -> LayoutMetrics:
    sm = [sheet_metrics(s) for s in sheets]
    utils = sorted((m.util_pct for m in sm), reverse=True)
    lead = sum(utils[:-1]) / (len(utils) - 1) if len(utils) > 1 else (utils[0] if utils else 0.0)
    min_util = min(utils) if utils else 0.0
    total_waste = sum(m.waste_area_mm2 for m in sm)
    secondary = sum(m.secondary_waste_area_mm2 for m in sm)
    internal = sum(m.internal_void_area_mm2 for m in sm)
    thin = sum(m.thin_waste_area_mm2 for m in sm)
    largest_boundary = sum(m.largest_boundary_zone_area_mm2 for m in sm)
    largest_corner = sum(m.largest_corner_zone_area_mm2 for m in sm)
    total_zones = sum(m.zones for m in sm)
    extra_zones = sum(max(0, m.zones - 1) for m in sm)
    densities = [m.cluster_bbox_density for m in sm if m.cluster_bbox_density > 0]
    secondary_ratio = secondary / total_waste if total_waste > 0 else 0.0
    internal_ratio = internal / total_waste if total_waste > 0 else 0.0
    thin_ratio = thin / total_waste if total_waste > 0 else 0.0
    boundary_ratio = largest_boundary / total_waste if total_waste > 0 else 0.0
    corner_ratio = largest_corner / total_waste if total_waste > 0 else 0.0
    avg_density = sum(densities) / len(densities) if densities else 0.0
    min_density = min(densities) if densities else 0.0
    part_contact = sum(m.part_contact_mm for m in sm)
    part_contact_ratio = sum(m.part_contact_ratio for m in sm) / len(sm) if sm else 0.0
    visual_loss = (
        1000.0 * max(0, len(sheets) - 4)
        + 80.0 * extra_zones
        + 60.0 * secondary_ratio
        + 45.0 * internal_ratio
        + 25.0 * thin_ratio
        - 30.0 * boundary_ratio
        - 20.0 * corner_ratio
        - 15.0 * avg_density
        - 25.0 * part_contact_ratio
    )
    return LayoutMetrics(
        label=label,
        source=str(source.relative_to(ROOT)),
        sheets=len(sheets),
        lead_util_pct=round(lead, 3),
        min_util_pct=round(min_util, 3),
        total_zones=total_zones,
        extra_zones=extra_zones,
        max_zones_per_sheet=max((m.zones for m in sm), default=0),
        total_waste_area_mm2=round(total_waste, 1),
        secondary_waste_area_mm2=round(secondary, 1),
        secondary_waste_ratio=round(secondary_ratio, 5),
        internal_void_area_mm2=round(internal, 1),
        internal_void_ratio=round(internal_ratio, 5),
        thin_waste_area_mm2=round(thin, 1),
        thin_waste_ratio=round(thin_ratio, 5),
        largest_boundary_zone_ratio=round(boundary_ratio, 5),
        largest_corner_zone_ratio=round(corner_ratio, 5),
        avg_cluster_bbox_density=round(avg_density, 5),
        min_cluster_bbox_density=round(min_density, 5),
        internal_gap_area_mm2=round(sum(m.internal_gap_area_mm2 for m in sm), 1),
        part_contact_mm=round(part_contact, 1),
        part_contact_ratio=round(part_contact_ratio, 5),
        visual_loss=round(visual_loss, 3),
        per_sheet_zones=[m.zones for m in sm],
        per_sheet_util_pct=[round(m.util_pct, 2) for m in sm],
    )


def candidate_inputs() -> list[tuple[str, Path]]:
    paths: list[tuple[str, Path]] = []
    for path in sorted((ROOT / "ai_docs" / "tmp" / "v41c_visual_artifacts").glob("*.json")):
        paths.append((path.stem, path))
    v31 = ROOT / "ai_docs" / "tmp" / "best_layouts_v31_paired_group_shift_diff_quick"
    for name in [
        "seed_08_moves4_closed498280_before.svg",
        "seed_08_moves4_closed498280_after.svg",
    ]:
        path = v31 / name
        if path.exists():
            paths.append((path.stem, path))
    v30 = ROOT / "ai_docs" / "tmp" / "best_layouts_v30_group_shift_metrics_quick"
    for name in ["off_seed2.json", "on_seed2.json"]:
        path = v30 / name
        if path.exists():
            paths.append((path.stem, path))
    return paths


def write_outputs(rows: list[LayoutMetrics]) -> None:
    with (OUT_DIR / "v42_metrics.json").open("w", encoding="utf-8") as f:
        json.dump([asdict(r) for r in rows], f, indent=2, ensure_ascii=False)

    with (OUT_DIR / "v42_metrics.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=list(asdict(rows[0]).keys()))
        writer.writeheader()
        for row in rows:
            data = asdict(row)
            data["per_sheet_zones"] = "/".join(map(str, row.per_sheet_zones))
            data["per_sheet_util_pct"] = "/".join(f"{v:.2f}" for v in row.per_sheet_util_pct)
            writer.writerow(data)

    pairs = [
        ("v41c_seed11", "seed_11_old", "seed_11_new"),
        ("v41c_seed13", "seed_13_old", "seed_13_new"),
        (
            "v31_group_shift_seed08",
            "seed_08_moves4_closed498280_before",
            "seed_08_moves4_closed498280_after",
        ),
        ("v30_group_shift_seed2", "off_seed2", "on_seed2"),
    ]
    by_label = {row.label: row for row in rows}
    lines = [
        "# V42 visual/remnant metrics audit",
        "",
        f"Cell: {CELL_MM} mm. Min zone area: {MIN_ZONE_AREA_MM2:.0f} mm2. "
        f"Paired lead guard: {LEAD_DROP_GUARD_PP:.2f}pp.",
        "",
        "| Case | Sheets | Lead | Zones | Secondary ratio | Internal ratio | Boundary ratio | Cluster density | Contact ratio | Visual loss |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row.label} | {row.sheets} | {row.lead_util_pct:.2f}% | "
            f"{row.total_zones} ({'/'.join(map(str, row.per_sheet_zones))}) | "
            f"{row.secondary_waste_ratio:.3f} | {row.internal_void_ratio:.3f} | "
            f"{row.largest_boundary_zone_ratio:.3f} | {row.avg_cluster_bbox_density:.3f} | "
            f"{row.part_contact_ratio:.3f} | {row.visual_loss:.2f} |"
        )
    lines.extend(["", "## Paired deltas", ""])
    lines.append("| Pair | Zones delta | Lead delta | Secondary delta | Internal delta | Contact delta | Loss delta | Read |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---|")
    for pair, old_label, new_label in pairs:
        if old_label not in by_label or new_label not in by_label:
            continue
        old = by_label[old_label]
        new = by_label[new_label]
        dz = new.total_zones - old.total_zones
        dlead = new.lead_util_pct - old.lead_util_pct
        dsec = new.secondary_waste_ratio - old.secondary_waste_ratio
        dint = new.internal_void_ratio - old.internal_void_ratio
        dcontact = new.part_contact_ratio - old.part_contact_ratio
        dloss = new.visual_loss - old.visual_loss
        lead_guard_failed = dlead < -LEAD_DROP_GUARD_PP
        if dz < 0 and lead_guard_failed:
            read = "zones improve but lead guard fails; requires visual review"
        elif dz < 0 and dloss < 0:
            read = "metric agrees with visual improvement"
        elif dz < 0 and dloss >= 0:
            read = "zones improve, combined score warns about trade-off"
        elif dz == 0 and dloss < 0:
            read = "shape improves without zone-count change"
        elif dz == 0 and dcontact > 0:
            read = "part-contact improves without zone-count change"
        else:
            read = "no clear improvement"
        lines.append(
            f"| {pair} | {dz:+d} | {dlead:+.2f}pp | {dsec:+.3f} | {dint:+.3f} | "
            f"{dcontact:+.3f} | {dloss:+.2f} | {read} |"
        )
    (OUT_DIR / "v42_summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    rows = []
    for label, path in candidate_inputs():
        try:
            sheets = load_layout(path)
            rows.append(layout_metrics(label, path, sheets))
        except Exception as exc:  # pragma: no cover - benchmark script diagnostics
            print(f"FAILED {label}: {exc}", flush=True)
    rows.sort(key=lambda r: r.label)
    if not rows:
        raise SystemExit("No layouts found for V42 audit")
    write_outputs(rows)
    print(f"wrote {len(rows)} rows to {OUT_DIR}", flush=True)
    for row in rows:
        print(
            f"{row.label}: sheets={row.sheets} lead={row.lead_util_pct:.2f}% "
            f"zones={row.total_zones} loss={row.visual_loss:.2f}",
            flush=True,
        )


if __name__ == "__main__":
    main()
