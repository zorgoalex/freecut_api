"""V23 visual audit: compare V20 and V22 waste-shape tradeoffs.

The V22 default profile pool improves waste-region counts, but its average
largest reusable corner is lower than V20.  This script compares the saved
30-seed summaries seed-by-seed and creates a simple contact sheet from the
top SVG artifacts for visual inspection.
"""

import csv
import json
import os
import re
import statistics
import sys
import xml.etree.ElementTree as ET
from collections import Counter
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
V20_DIR = ARTIFACT_ROOT / "best_layouts_v20_profile_pool_zp02_guard08_30sweep"
V22_DIR = ARTIFACT_ROOT / "best_layouts_v22_profile_pool_zp04_guard08_30sweep"
OUT_DIR = ARTIFACT_ROOT / "best_layouts_v23_visual_corner_audit"
OUT_DIR.mkdir(parents=True, exist_ok=True)


def load_summary(path: Path) -> dict:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def rows(summary: dict) -> list[dict]:
    return list(summary.get("results", []))


def mean(values: list[float]) -> float:
    return round(statistics.fmean(values), 3) if values else 0.0


def aggregate(rows_: list[dict]) -> dict:
    zones = [int(row["n_waste_regions"]) for row in rows_]
    corners = [float(row["max_corner_mm2"]) for row in rows_]
    leads = [float(row["lead_util"]) for row in rows_]
    sheets = [int(row["sheets"]) for row in rows_]
    candidates = [int(row.get("candidates_completed", 0)) for row in rows_]
    min_utils = [min(row.get("utils", [0])) for row in rows_]
    profiles = Counter(str(row.get("zone_penalty")) for row in rows_)

    return {
        "count": len(rows_),
        "four_sheet_count": sum(1 for value in sheets if value == 4),
        "avg_zones": mean(zones),
        "min_zones": min(zones),
        "max_zones": max(zones),
        "zones_le_4": sum(1 for value in zones if value <= 4),
        "zones_le_5": sum(1 for value in zones if value <= 5),
        "zones_gt_5": sum(1 for value in zones if value > 5),
        "avg_lead_util": mean(leads),
        "min_lead_util": min(leads),
        "avg_min_sheet_util": mean(min_utils),
        "avg_max_corner_mm2": round(mean(corners)),
        "min_max_corner_mm2": round(min(corners)),
        "max_max_corner_mm2": round(max(corners)),
        "corner_ge_300k": sum(1 for value in corners if value >= 300_000),
        "corner_ge_400k": sum(1 for value in corners if value >= 400_000),
        "corner_ge_500k": sum(1 for value in corners if value >= 500_000),
        "avg_candidates_completed": mean(candidates),
        "profile_counts": dict(sorted(profiles.items())),
    }


def build_deltas(v20_rows: list[dict], v22_rows: list[dict]) -> list[dict]:
    v20_by_seed = {int(row["seed"]): row for row in v20_rows}
    v22_by_seed = {int(row["seed"]): row for row in v22_rows}
    deltas = []
    for seed in sorted(set(v20_by_seed) & set(v22_by_seed)):
        old = v20_by_seed[seed]
        new = v22_by_seed[seed]
        delta_zones = int(new["n_waste_regions"]) - int(old["n_waste_regions"])
        delta_corner = int(new["max_corner_mm2"]) - int(old["max_corner_mm2"])
        delta_lead = round(float(new["lead_util"]) - float(old["lead_util"]), 3)
        if delta_zones < 0 and delta_corner < -100_000:
            verdict = "zones_better_corner_cost"
        elif delta_zones < 0:
            verdict = "zones_better"
        elif delta_zones == 0 and delta_corner < -100_000:
            verdict = "same_zones_corner_cost"
        elif delta_zones == 0 and delta_corner > 100_000:
            verdict = "same_zones_corner_better"
        elif delta_zones > 0:
            verdict = "zones_worse"
        else:
            verdict = "neutral"
        deltas.append(
            {
                "seed": seed,
                "v20_zones": int(old["n_waste_regions"]),
                "v22_zones": int(new["n_waste_regions"]),
                "delta_zones": delta_zones,
                "v20_lead_util": float(old["lead_util"]),
                "v22_lead_util": float(new["lead_util"]),
                "delta_lead_util": delta_lead,
                "v20_max_corner_mm2": int(old["max_corner_mm2"]),
                "v22_max_corner_mm2": int(new["max_corner_mm2"]),
                "delta_max_corner_mm2": delta_corner,
                "v20_zone_penalty": old.get("zone_penalty"),
                "v22_zone_penalty": new.get("zone_penalty"),
                "v20_min_sheet_util": min(old.get("utils", [0])),
                "v22_min_sheet_util": min(new.get("utils", [0])),
                "verdict": verdict,
            }
        )
    return deltas


def write_csv(path: Path, items: list[dict]) -> None:
    if not items:
        return
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(items[0].keys()))
        writer.writeheader()
        writer.writerows(items)


def svg_sort_key(path: Path) -> tuple[int, str]:
    match = re.search(r"rank_(\d+)", path.name)
    rank = int(match.group(1)) if match else 999
    return rank, path.name


def parse_color(value: str | None) -> tuple[int, int, int] | None:
    if not value or value == "none" or not value.startswith("#"):
        return None
    value = value.lstrip("#")
    if len(value) == 3:
        value = "".join(ch * 2 for ch in value)
    if len(value) != 6:
        return None
    return tuple(int(value[index : index + 2], 16) for index in (0, 2, 4))


def render_svg_to_image(svg_path: Path, width: int, height: int):
    from PIL import Image, ImageDraw

    root = ET.fromstring(svg_path.read_text(encoding="utf-8"))
    view_box = [float(value) for value in root.attrib["viewBox"].split()]
    min_x, min_y, box_w, box_h = view_box
    scale = min(width / box_w, height / box_h)
    draw_w = int(box_w * scale)
    draw_h = int(box_h * scale)
    offset_x = (width - draw_w) // 2
    offset_y = (height - draw_h) // 2
    image = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(image)

    def tx(value: float) -> int:
        return int(round(offset_x + (value - min_x) * scale))

    def ty(value: float) -> int:
        return int(round(offset_y + (value - min_y) * scale))

    for rect in root.iter("{http://www.w3.org/2000/svg}rect"):
        x = float(rect.attrib.get("x", 0))
        y = float(rect.attrib.get("y", 0))
        w = float(rect.attrib.get("width", 0))
        h = float(rect.attrib.get("height", 0))
        fill = parse_color(rect.attrib.get("fill")) or (255, 255, 255)
        stroke = parse_color(rect.attrib.get("stroke"))
        draw.rectangle([tx(x), ty(y), tx(x + w), ty(y + h)], fill=fill, outline=stroke)
    return image


def make_contact_sheet(v20_svg_dir: Path, v22_svg_dir: Path, out_path: Path) -> str | None:
    try:
        from PIL import Image, ImageDraw
    except Exception as exc:
        print(f"Contact sheet skipped: Pillow is not available ({exc})")
        return None

    v20_svgs = sorted(v20_svg_dir.glob("rank_*.svg"), key=svg_sort_key)[:5]
    v22_svgs = sorted(v22_svg_dir.glob("rank_*.svg"), key=svg_sort_key)[:5]
    count = max(len(v20_svgs), len(v22_svgs))
    if count == 0:
        return None

    cell_w = 300
    cell_h = 1450
    label_h = 42
    margin = 16
    sheet = Image.new(
        "RGB",
        (margin * 3 + cell_w * 2, margin * (count + 1) + (cell_h + label_h) * count),
        "white",
    )
    draw = ImageDraw.Draw(sheet)

    for row_index in range(count):
        y = margin + row_index * (cell_h + label_h + margin)
        for col_index, (label, svgs) in enumerate((("V20", v20_svgs), ("V22", v22_svgs))):
            x = margin + col_index * (cell_w + margin)
            title = f"{label}: {svgs[row_index].name if row_index < len(svgs) else 'missing'}"
            draw.text((x, y), title, fill=(30, 30, 30))
            draw.rectangle(
                [x, y + label_h - 8, x + cell_w, y + label_h + cell_h],
                outline=(160, 160, 160),
                width=1,
            )
            if row_index < len(svgs):
                rendered = render_svg_to_image(svgs[row_index], cell_w, cell_h)
                sheet.paste(rendered, (x, y + label_h))

    sheet.save(out_path)
    return str(out_path)


def main() -> None:
    v20_summary = load_summary(V20_DIR / "v17b_profile_pool_service_summary.json")
    v22_summary = load_summary(V22_DIR / "v17b_profile_pool_service_summary.json")
    v20_rows = rows(v20_summary)
    v22_rows = rows(v22_summary)
    deltas = build_deltas(v20_rows, v22_rows)

    analysis = {
        "inputs": {
            "artifact_root": str(ARTIFACT_ROOT),
            "v20_dir": str(V20_DIR),
            "v22_dir": str(V22_DIR),
            "v20_profiles": v20_summary.get("profiles"),
            "v22_profiles": v22_summary.get("profiles"),
        },
        "aggregate": {
            "v20": aggregate(v20_rows),
            "v22": aggregate(v22_rows),
        },
        "delta_summary": {
            "zones_improved": sum(1 for item in deltas if item["delta_zones"] < 0),
            "zones_same": sum(1 for item in deltas if item["delta_zones"] == 0),
            "zones_worse": sum(1 for item in deltas if item["delta_zones"] > 0),
            "corner_improved_100k": sum(
                1 for item in deltas if item["delta_max_corner_mm2"] >= 100_000
            ),
            "corner_regressed_100k": sum(
                1 for item in deltas if item["delta_max_corner_mm2"] <= -100_000
            ),
            "lead_regressed_0_2pp": sum(
                1 for item in deltas if item["delta_lead_util"] <= -0.2
            ),
            "v22_corner_below_300k": sum(
                1 for item in deltas if item["v22_max_corner_mm2"] < 300_000
            ),
            "v22_corner_ge_300k": sum(
                1 for item in deltas if item["v22_max_corner_mm2"] >= 300_000
            ),
            "v22_corner_ge_400k": sum(
                1 for item in deltas if item["v22_max_corner_mm2"] >= 400_000
            ),
        },
        "zone_improvements": [item for item in deltas if item["delta_zones"] < 0],
        "new_four_zone_winners": [
            item
            for item in deltas
            if item["v20_zones"] > 4 and item["v22_zones"] <= 4
        ],
        "corner_regressions_100k": [
            item for item in deltas if item["delta_max_corner_mm2"] <= -100_000
        ],
        "same_zones_corner_regressions_100k": [
            item
            for item in deltas
            if item["delta_zones"] == 0 and item["delta_max_corner_mm2"] <= -100_000
        ],
        "deltas": deltas,
    }

    write_csv(OUT_DIR / "v23_v20_v22_seed_deltas.csv", deltas)
    with (OUT_DIR / "v23_visual_corner_audit_summary.json").open(
        "w", encoding="utf-8"
    ) as handle:
        json.dump(analysis, handle, ensure_ascii=False, indent=2)

    contact_sheet = make_contact_sheet(
        V20_DIR, V22_DIR, OUT_DIR / "v23_v20_v22_top5_contact_sheet.png"
    )
    if contact_sheet:
        analysis["contact_sheet"] = contact_sheet
        with (OUT_DIR / "v23_visual_corner_audit_summary.json").open(
            "w", encoding="utf-8"
        ) as handle:
            json.dump(analysis, handle, ensure_ascii=False, indent=2)

    print("V23 visual/corner audit")
    print(f"artifact_root={ARTIFACT_ROOT}")
    for name in ("v20", "v22"):
        data = analysis["aggregate"][name]
        print(
            f"{name}: avg_zones={data['avg_zones']}, <=4={data['zones_le_4']}/30, "
            f"<=5={data['zones_le_5']}/30, lead={data['avg_lead_util']}%, "
            f"avg_corner={data['avg_max_corner_mm2']}, corner>=300k={data['corner_ge_300k']}/30"
        )
    print(f"delta_summary={analysis['delta_summary']}")
    if contact_sheet:
        print(f"contact_sheet={contact_sheet}")
    print(f"summary={OUT_DIR / 'v23_visual_corner_audit_summary.json'}")
    print(f"csv={OUT_DIR / 'v23_v20_v22_seed_deltas.csv'}")


if __name__ == "__main__":
    main()
