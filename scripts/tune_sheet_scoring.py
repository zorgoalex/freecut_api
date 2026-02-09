#!/usr/bin/env python3
import argparse
import json
import math
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple

from optimize_search import calculate_internal_void_metrics


@dataclass(frozen=True)
class LabeledSheet:
    group: str  # best|medium|bad|very_bad
    svg_name: str
    sheet_no: int  # 1-based (как в SVG: сверху вниз)


GROUP_ALIASES: Dict[str, str] = {
    "best": "Лучшие",
    "medium": "Средние",
    "bad": "Плохие",
    "very_bad": "Очень плохие",
}


def detect_group(line: str) -> Optional[str]:
    raw = line.strip().lower()
    if raw.startswith("лучшие конкретные листы"):
        return "best"
    if raw.startswith("конкретные листы раскладок"):
        return "medium"
    if raw.startswith("плохие конкретные листы"):
        return "bad"
    if raw.startswith("конкретные листы с очень плохой"):
        return "very_bad"
    return None


LINE_RE = re.compile(r"(?P<file>\S+\.svg)\s*-\s*(?P<sheets>[0-9,\s]+)\s*лист\w*")


def parse_last_task(text: str) -> List[LabeledSheet]:
    current_group: Optional[str] = None
    items: List[LabeledSheet] = []

    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue

        group = detect_group(line)
        if group is not None:
            current_group = group
            continue

        if current_group is None:
            continue

        m = LINE_RE.search(line)
        if not m:
            continue

        svg_name = m.group("file").strip()
        sheets_raw = m.group("sheets")
        sheet_nos: List[int] = []
        for part in sheets_raw.split(","):
            part = part.strip()
            if not part:
                continue
            sheet_nos.append(int(part))

        for sheet_no in sheet_nos:
            items.append(LabeledSheet(group=current_group, svg_name=svg_name, sheet_no=sheet_no))

    return items


def load_text(path: Path) -> str:
    # last_task.md в репозитории может быть с BOM, поэтому читаем как utf-8-sig.
    return path.read_text(encoding="utf-8-sig")


def load_json(path: Path) -> Dict[str, Any]:
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def load_fixture_params(path: Path) -> Tuple[float, float]:
    body = load_json(path)
    params = body.get("params", {}) or {}
    spacing_mm = float(params.get("spacing_mm", 0.0) or 0.0)
    kerf_mm = float(params.get("kerf_mm", 0.0) or 0.0)
    return spacing_mm, kerf_mm


def resolve_json_artifact(artifacts_dir: Path, svg_name: str) -> Path:
    stem = Path(svg_name).stem
    direct = artifacts_dir / f"{stem}.json"
    if direct.exists():
        return direct
    matches = sorted(p for p in artifacts_dir.glob("*.json") if p.name.endswith(f"{stem}.json"))
    if not matches:
        raise FileNotFoundError(f"json for {svg_name} not found in {artifacts_dir}")
    return matches[0]


def bbox_metrics(placements: List[Dict[str, Any]]) -> Optional[Dict[str, float]]:
    if not placements:
        return None
    min_x = min(float(p["x_mm"]) for p in placements)
    min_y = min(float(p["y_mm"]) for p in placements)
    max_x = max(float(p["x_mm"]) + float(p["width_mm"]) for p in placements)
    max_y = max(float(p["y_mm"]) + float(p["height_mm"]) for p in placements)
    area = sum(float(p["width_mm"]) * float(p["height_mm"]) for p in placements)
    return {"min_x": min_x, "min_y": min_y, "max_x": max_x, "max_y": max_y, "area": area}


def sheet_metrics(
    body: Dict[str, Any],
    sheet_no: int,
    grid_mm: float,
    spacing_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
    solutions: List[Dict[str, Any]] = body.get("solutions", []) or []
    idx = int(sheet_no) - 1
    if idx < 0 or idx >= len(solutions):
        raise IndexError(f"sheet_no={sheet_no} out of range (solutions={len(solutions)})")
    sol = solutions[idx]
    placements = sol.get("placements", []) or []

    trim = sol.get("trim_mm") or {}
    usable_w = float(sol["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    usable_h = float(sol["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    usable_area = max(0.0, usable_w) * max(0.0, usable_h)

    bbox = bbox_metrics(placements)
    placed_area = float(bbox["area"]) if bbox else 0.0
    placed_count = len(placements)
    used_ratio = (placed_area / usable_area) if usable_area > 0 else 0.0

    (
        internal_void_mm2,
        internal_components,
        exposure_penalty,
        corridor_void_mm2,
        corridor_components,
        row_gap_mm2,
        col_gap_mm2,
        corridor_weighted_mm2,
        max_penetration_mm,
        penetration_volume_mm3,
        penetration_weighted,
        occupied_perimeter_mm,
        void_compactness,
        edge_continuity,
        edge_breaks,
        corners_filled,
    ) = calculate_internal_void_metrics(
        solutions=[sol],
        grid_mm=float(grid_mm),
        pad_mm=0.0,
        spacing_mm=float(spacing_mm),
        corridor_ok_mult=float(corridor_ok_mult),
    )

    bbox_area = 0.0
    bbox_void_mm2 = 0.0
    bbox_edge_min_mm = 0.0
    edges_touched = 0
    if bbox:
        bbox_w = max(0.0, float(bbox["max_x"]) - float(bbox["min_x"]))
        bbox_h = max(0.0, float(bbox["max_y"]) - float(bbox["min_y"]))
        bbox_area = bbox_w * bbox_h
        bbox_void_mm2 = max(0.0, bbox_area - placed_area)

        d_left = float(bbox["min_x"])
        d_top = float(bbox["min_y"])
        d_right = max(0.0, usable_w - float(bbox["max_x"]))
        d_bottom = max(0.0, usable_h - float(bbox["max_y"]))
        bbox_edge_min_mm = min(d_left, d_top, d_right, d_bottom)
        tol = max(1.0, float(grid_mm))
        edges_touched = sum(1 for d in (d_left, d_top, d_right, d_bottom) if d <= tol)

    return {
        "sheet_no": sheet_no,
        "placed_count": placed_count,
        "placed_area_mm2": placed_area,
        "used_ratio": used_ratio,
        "bbox_area_mm2": bbox_area,
        "bbox_void_mm2": bbox_void_mm2,
        "bbox_edge_min_mm": bbox_edge_min_mm,
        "bbox_edges_touched": edges_touched,
        "internal_void_mm2": float(internal_void_mm2),
        "internal_components": int(internal_components),
        "exposure_penalty": float(exposure_penalty),
        "corridor_void_mm2": float(corridor_void_mm2),
        "corridor_components": int(corridor_components),
        "corridor_weighted_mm2": float(corridor_weighted_mm2),
        "row_gap_mm2": float(row_gap_mm2),
        "col_gap_mm2": float(col_gap_mm2),
        "max_penetration_mm": float(max_penetration_mm),
        "penetration_volume_mm3": float(penetration_volume_mm3),
        "penetration_weighted": float(penetration_weighted),
        "occupied_perimeter_mm": float(occupied_perimeter_mm),
        "void_compactness": float(void_compactness),
        "edge_continuity": float(edge_continuity),
        "edge_breaks": int(edge_breaks),
        "corners_filled": int(corners_filled),
    }


def fmt_float(v: float, digits: int = 3) -> str:
    if math.isfinite(v):
        return f"{v:.{digits}f}"
    return "inf"


def visual_sort_key(m: Dict[str, Any]) -> Tuple[Any, ...]:
    # Лексикографический ключ под "визуальное качество" (лучше -> меньше).
    # Важное: internal_void не ставим первым, т.к. на практике он может быть небольшим и не доминировать
    # визуальную оценку (в отличие от больших коридоров/центрированных деталей).
    return (
        float(m["exposure_penalty"]),
        float(m["penetration_weighted"]),
        float(m["corridor_void_mm2"]),
        float(m["occupied_perimeter_mm"]),
        int(m["corridor_components"]),
        int(m["edge_breaks"]),
        float(m["internal_void_mm2"]),
        float(1.0 - float(m["used_ratio"])),
    )


def visual_score(m: Dict[str, Any]) -> float:
    # Скаляр для удобства: выше = лучше.
    # Нормализация подобрана под текущий датасет `last_task.md` (multisheet_varied_4sheets):
    # - exposure_penalty: ~3e7..2e9
    # - penetration_weighted: ~0..1.6e10
    # - corridor_void_mm2: ~0..1e6
    # - occupied_perimeter_mm: ~4e3..3.3e4
    penalty = 0.0
    penalty += float(m["exposure_penalty"]) / 1e8
    # penetration_weighted имеет очень большой диапазон и легко доминирует,
    # поэтому даём ему меньший вес, чтобы не "перебивать" очевидно плохие (пустые/центрированные) листы.
    penalty += float(m["penetration_weighted"]) / 5e9
    penalty += float(m["corridor_void_mm2"]) / 1e6
    penalty += float(m["occupied_perimeter_mm"]) / 1e4
    penalty += float(m["edge_breaks"]) / 2000.0
    penalty += float(m["internal_void_mm2"]) / 1e5
    penalty += 10.0 * float(1.0 - float(m["used_ratio"]))
    return -float(penalty)


def count_by_group(items: Iterable[LabeledSheet]) -> Dict[str, int]:
    out: Dict[str, int] = {}
    for it in items:
        out[it.group] = out.get(it.group, 0) + 1
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="Parse ai_docs/last_task.md into a labeled per-sheet dataset.")
    ap.add_argument("--last-task", default="ai_docs/last_task.md")
    ap.add_argument("--dump-json", action="store_true", help="Dump dataset as JSON to stdout.")
    ap.add_argument("--metrics", action="store_true", help="Compute per-sheet metrics using *.json artifacts.")
    ap.add_argument("--rank", action="store_true", help="Print ranked list by visual_sort_key/visual_score.")
    ap.add_argument("--artifacts-dir", default="ai_docs/tmp/placement_bias_sweep_nested_for_scoring")
    ap.add_argument("--fixture", default="tests/fixtures/multisheet_varied_4sheets.json")
    ap.add_argument("--grid-mm", type=float, default=5.0)
    ap.add_argument("--corridor-ok-mult", type=float, default=3.0)
    args = ap.parse_args()

    last_task_path = Path(args.last_task)
    text = load_text(last_task_path)
    items = parse_last_task(text)

    if args.dump_json:
        print(json.dumps([it.__dict__ for it in items], ensure_ascii=False, indent=2))
        return 0

    counts = count_by_group(items)
    total = sum(counts.values())
    uniq_svgs = sorted({it.svg_name for it in items})
    print(f"last_task: {last_task_path}")
    print(f"entries: {total}")
    for group in ("best", "medium", "bad", "very_bad"):
        print(f"- {GROUP_ALIASES[group]}: {counts.get(group, 0)}")
    print(f"unique svgs: {len(uniq_svgs)}")
    for name in uniq_svgs:
        print(f"  - {name}")

    if not args.metrics:
        return 0

    artifacts_dir = Path(args.artifacts_dir)
    fixture_path = Path(args.fixture)
    spacing_mm, _kerf_mm = load_fixture_params(fixture_path)

    print("")
    print("Per-sheet metrics (one row per labeled sheet):")
    header = [
        "group",
        "svg",
        "sheet",
        "placed",
        "used_ratio",
        "internal_void",
        "corr_void",
        "corr_comp",
        "perim",
        "vcomp",
        "edge_cont",
        "corners",
        "bbox_edge_min",
        "bbox_edges_touch",
    ]
    print(" | ".join(header))
    print("-" * 120)

    rows: List[Dict[str, Any]] = []
    for it in items:
        json_path = resolve_json_artifact(artifacts_dir, it.svg_name)
        body = load_json(json_path)
        m = sheet_metrics(
            body=body,
            sheet_no=it.sheet_no,
            grid_mm=args.grid_mm,
            spacing_mm=spacing_mm,
            corridor_ok_mult=args.corridor_ok_mult,
        )
        row = {
            "group": it.group,
            "svg_name": Path(json_path).with_suffix(".svg").name,
            "sheet_no": it.sheet_no,
            **m,
            "visual_score": 0.0,
        }
        row["visual_score"] = visual_score(row)
        rows.append(row)
        print(
            " | ".join(
                [
                    f"{GROUP_ALIASES[it.group]:<11}",
                    f"{row['svg_name']:<64}",
                    f"{it.sheet_no:<5}",
                    f"{row['placed_count']:<6}",
                    fmt_float(float(row["used_ratio"]), 3),
                    fmt_float(float(row["internal_void_mm2"]), 0),
                    fmt_float(float(row["corridor_void_mm2"]), 0),
                    f"{int(row['corridor_components']):<9}",
                    fmt_float(float(row["occupied_perimeter_mm"]), 0),
                    fmt_float(float(row["void_compactness"]), 4),
                    fmt_float(float(row["edge_continuity"]), 3),
                    f"{int(row['corners_filled']):<7}",
                    fmt_float(float(row["bbox_edge_min_mm"]), 1),
                    f"{int(row['bbox_edges_touched']):<15}",
                ]
            )
        )

    if args.rank:
        print("")
        print("Ranked (best -> worst):")
        ranked = sorted(rows, key=lambda r: (visual_sort_key(r), -float(r["visual_score"])))
        for idx, r in enumerate(ranked, 1):
            print(
                f"{idx:>2}. {GROUP_ALIASES[r['group']]:<11} sheet={r['sheet_no']} "
                f"score={float(r['visual_score']):.3f} "
                f"exp={float(r['exposure_penalty']):.0f} penw={float(r['penetration_weighted']):.0f} "
                f"corr={float(r['corridor_void_mm2']):.0f} perim={float(r['occupied_perimeter_mm']):.0f} "
                f"used={float(r['used_ratio']):.3f} "
                f"svg={r['svg_name']}"
            )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
