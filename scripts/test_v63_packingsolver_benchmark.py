#!/usr/bin/env python3
"""V63 benchmark: Freecut heuristic vs PackingSolver rectangleguillotine.

This is a research script, not production integration. It:
- converts a Freecut OptimizeRequest fixture into PackingSolver CSV files;
- runs the current Freecut service with params.engine=heuristic;
- runs PackingSolver rectangleguillotine in tree-search-only bin-packing mode;
- stores JSON/CSV/SVG artifacts under ai_docs/tmp.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = ROOT


def find_artifact_root() -> Path:
    """Return the repo root whose ai_docs/tmp should hold shared artifacts."""
    current = Path(__file__).resolve()
    marker = Path("ai_docs") / "tmp" / "external" / "packingsolver"
    for parent in current.parents:
        if (parent / marker).exists():
            return parent
    return REPO_ROOT


ARTIFACT_ROOT = find_artifact_root()


def repo_tmp(*parts: str) -> Path:
    return ARTIFACT_ROOT / "ai_docs" / "tmp" / Path(*parts)


def default_solver_exe() -> Path:
    return repo_tmp(
        "external",
        "packingsolver",
        "build_v63_vs",
        "src",
        "rectangleguillotine",
        "Release",
        "packingsolver_rectangleguillotine.exe",
    )


def load_fixture(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def repeat_fixture(req: dict[str, Any], factor: int) -> dict[str, Any]:
    if factor <= 1:
        return req
    repeated = json.loads(json.dumps(req))
    for item in repeated["items"]:
        item["qty"] = int(item["qty"]) * factor
    repeated["params"]["seed"] = int(repeated["params"].get("seed") or 12345)
    return repeated


def as_int_scaled(value: float, scale: int) -> int:
    return int(round(float(value) * scale))


def expand_items(req: dict[str, Any]) -> list[dict[str, Any]]:
    expanded: list[dict[str, Any]] = []
    for item_type in req["items"]:
        for copy_idx in range(int(item_type["qty"])):
            expanded.append(
                {
                    "type_id": item_type["id"],
                    "copy_idx": copy_idx,
                    "width_mm": float(item_type["width_mm"]),
                    "height_mm": float(item_type["height_mm"]),
                    "rotation": item_type.get("rotation", "allow_90"),
                }
            )
    return expanded


def write_packingsolver_inputs(req: dict[str, Any], out_dir: Path, scale: int) -> dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    items_path = out_dir / "items.csv"
    bins_path = out_dir / "bins.csv"

    with items_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["ID", "WIDTH", "HEIGHT", "COPIES", "ORIENTED"])
        for idx, item in enumerate(req["items"]):
            oriented = 1 if item.get("rotation") == "forbid" else 0
            writer.writerow(
                [
                    idx,
                    as_int_scaled(item["width_mm"], scale),
                    as_int_scaled(item["height_mm"], scale),
                    int(item["qty"]),
                    oriented,
                ]
            )

    stock = req["stock"][0]
    params = req["params"]
    trim = params["trim_mm"]
    stock_qty = int(stock.get("qty", 0))
    if stock_qty <= 0:
        stock_qty = max(1, sum(int(item["qty"]) for item in req["items"]))

    with bins_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(
            [
                "ID",
                "WIDTH",
                "HEIGHT",
                "COPIES",
                "LEFT_TRIM",
                "RIGHT_TRIM",
                "BOTTOM_TRIM",
                "TOP_TRIM",
                "LEFT_TRIM_TYPE",
                "RIGHT_TRIM_TYPE",
                "BOTTOM_TRIM_TYPE",
                "TOP_TRIM_TYPE",
            ]
        )
        writer.writerow(
            [
                0,
                as_int_scaled(stock["width_mm"], scale),
                as_int_scaled(stock["height_mm"], scale),
                stock_qty,
                as_int_scaled(trim["left"], scale),
                as_int_scaled(trim["right"], scale),
                as_int_scaled(trim["bottom"], scale),
                as_int_scaled(trim["top"], scale),
                "hard",
                "hard",
                "hard",
                "hard",
            ]
        )

    return {
        "items_path": str(items_path),
        "bins_path": str(bins_path),
        "stock_qty": stock_qty,
        "scale": scale,
    }


def wait_ready(port: int, timeout_s: float) -> None:
    deadline = time.time() + timeout_s
    url = f"http://127.0.0.1:{port}/health/ready"
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1.0) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(0.25)
    raise RuntimeError(f"Freecut service did not become ready at {url}")


def post_json(url: str, payload: dict[str, Any], timeout_s: float) -> dict[str, Any]:
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout_s) as resp:
        return json.loads(resp.read().decode("utf-8"))


def summarize_freecut_response(response: dict[str, Any], request_wall_ms: float) -> dict[str, Any]:
    summary = response.get("summary", {})
    return {
        "status": response.get("status"),
        "request_wall_ms": request_wall_ms,
        "api_time_ms": summary.get("time_ms"),
        "used_stock_count": summary.get("used_stock_count"),
        "waste_percent": summary.get("waste_percent"),
        "unplaced_count": len(response.get("unplaced", [])),
        "summary": summary,
    }


def write_freecut_artifacts(out_dir: Path, name: str, response: dict[str, Any]) -> None:
    (out_dir / f"{name}_response.json").write_text(
        json.dumps(response, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    svg = response.get("artifact_svg")
    if svg:
        (out_dir / f"{name}.svg").write_text(svg, encoding="utf-8")


def run_freecut_baselines(req: dict[str, Any], port: int, out_dir: Path) -> dict[str, Any]:
    scenarios: list[tuple[str, dict[str, Any]]] = []

    default_payload = json.loads(json.dumps(req))
    default_payload["params"]["include_svg"] = True
    scenarios.append(("freecut_fixture_defaults", default_payload))

    heuristic_payload = json.loads(json.dumps(req))
    heuristic_payload["params"]["engine"] = "heuristic"
    heuristic_payload["params"]["include_svg"] = True
    heuristic_payload["params"]["retry_strategy"] = "disabled"
    heuristic_payload["params"]["time_limit_ms"] = 100
    heuristic_payload["params"]["restarts"] = 1
    scenarios.append(("freecut_heuristic_fast", heuristic_payload))

    env = os.environ.copy()
    env["PORT"] = str(port)
    proc = subprocess.Popen(
        ["cargo", "run", "--quiet"],
        cwd=REPO_ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    results: dict[str, Any] = {}
    try:
        wait_ready(port, 90.0)
        for name, payload in scenarios:
            start = time.perf_counter()
            response = post_json(f"http://127.0.0.1:{port}/v1/optimize", payload, 60.0)
            request_wall_ms = (time.perf_counter() - start) * 1000.0
            write_freecut_artifacts(out_dir, name, response)
            results[name] = summarize_freecut_response(response, request_wall_ms)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
    return results


def parse_packing_stdout(stdout: str) -> dict[str, Any]:
    metrics: dict[str, Any] = {}
    patterns = {
        "time_s": r"Time \(s\):\s+([0-9.]+)",
        "number_of_items": r"Number of items:\s+([0-9]+)\s*/\s*([0-9]+)",
        "number_of_bins": r"Number of bins:\s+([0-9]+)\s*/\s*([0-9]+)",
        "waste_percent": r"Full waste:\s+[0-9.]+\s+\(([0-9.]+)%\)",
        "feasible": r"Feasible:\s+([01])",
    }
    for key, pattern in patterns.items():
        match = re.search(pattern, stdout)
        if not match:
            continue
        if key in {"number_of_items", "number_of_bins"}:
            metrics[key] = int(match.group(1))
            metrics[key + "_total"] = int(match.group(2))
        elif key == "feasible":
            metrics[key] = bool(int(match.group(1)))
        else:
            metrics[key] = float(match.group(1))
    return metrics


def run_packingsolver(
    solver_exe: Path,
    out_dir: Path,
    time_limit_s: float,
    scale: int,
    req: dict[str, Any],
    packing_mode: str,
    lp_solver: str | None,
) -> dict[str, Any]:
    certificate_path = out_dir / "packingsolver_certificate.csv"
    stdout_path = out_dir / "packingsolver_stdout.txt"
    gap = float(req["params"]["kerf_mm"]) + float(req["params"]["spacing_mm"])
    cut_thickness = as_int_scaled(gap, scale)
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
    ]
    if packing_mode == "tree":
        cmd.extend(
            [
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
        )
    if lp_solver:
        cmd.extend(["--linear-programming-solver", lp_solver])
    start = time.perf_counter()
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=max(30.0, time_limit_s + 20.0),
    )
    wall_ms = (time.perf_counter() - start) * 1000.0
    stdout_path.write_text(proc.stdout, encoding="utf-8")
    metrics = parse_packing_stdout(proc.stdout)
    metrics.update(
        {
            "returncode": proc.returncode,
            "wall_ms": wall_ms,
            "cut_thickness_scaled": cut_thickness,
            "packing_mode": packing_mode,
            "lp_solver": lp_solver,
            "certificate_path": str(certificate_path),
        }
    )
    return metrics


def render_packingsolver_svg(req: dict[str, Any], out_dir: Path, scale: int) -> None:
    cert = out_dir / "packingsolver_certificate.csv"
    if not cert.exists():
        return
    item_names = [item["id"] for item in req["items"]]
    stock = req["stock"][0]
    sheet_w = float(stock["width_mm"])
    sheet_h = float(stock["height_mm"])
    rows: list[dict[str, Any]] = []
    with cert.open("r", encoding="utf-8", newline="") as f:
        for row in csv.DictReader(f):
            try:
                type_id = int(row["TYPE"])
            except ValueError:
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
    plates = sorted({r["plate"] for r in rows})
    gap = 40.0
    width = sheet_w
    height = len(plates) * sheet_h + (len(plates) - 1) * gap
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="#f7f7f7"/>',
    ]
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    for sheet_idx, plate in enumerate(plates):
        y0 = sheet_idx * (sheet_h + gap)
        parts.append(
            f'<rect x="0" y="{y0}" width="{sheet_w}" height="{sheet_h}" '
            'fill="#ffffff" stroke="#888" stroke-width="2"/>'
        )
        parts.append(
            f'<text x="10" y="{y0 + 24}" font-size="22" fill="#222">plate {plate}</text>'
        )
        for r in [row for row in rows if row["plate"] == plate]:
            color = colors[r["type_id"] % len(colors)]
            x = r["x"]
            y = y0 + r["y"]
            label = item_names[r["type_id"]] if r["type_id"] < len(item_names) else str(r["type_id"])
            parts.append(
                f'<rect x="{x:.3f}" y="{y:.3f}" width="{r["w"]:.3f}" height="{r["h"]:.3f}" '
                f'fill="{color}" stroke="#9a1515" stroke-width="2"/>'
            )
            parts.append(
                f'<text x="{x + 6:.3f}" y="{y + 18:.3f}" font-size="16" fill="#111">{label}</text>'
            )
    parts.append("</svg>")
    (out_dir / "packingsolver_layout.svg").write_text("\n".join(parts), encoding="utf-8")
    render_packingsolver_png(req, rows, plates, out_dir, sheet_w, sheet_h)


def render_packingsolver_png(
    req: dict[str, Any],
    rows: list[dict[str, Any]],
    plates: list[int],
    out_dir: Path,
    sheet_w: float,
    sheet_h: float,
) -> None:
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        return

    px_w = 1200
    px_per_mm = px_w / sheet_w
    gap_px = 28
    sheet_px_h = int(round(sheet_h * px_per_mm))
    px_h = len(plates) * sheet_px_h + max(0, len(plates) - 1) * gap_px
    img = Image.new("RGB", (px_w, px_h), "#f7f7f7")
    draw = ImageDraw.Draw(img)
    font = ImageFont.load_default()
    colors = ["#b7d7f0", "#d6ebc2", "#f7d6a5", "#d9c7f2", "#f4b6b6", "#c8e8df"]
    item_names = [item["id"] for item in req["items"]]
    for sheet_idx, plate in enumerate(plates):
        y0 = sheet_idx * (sheet_px_h + gap_px)
        draw.rectangle([0, y0, px_w - 1, y0 + sheet_px_h - 1], fill="#ffffff", outline="#777777")
        draw.text((8, y0 + 8), f"plate {plate}", fill="#222222", font=font)
        for r in [row for row in rows if row["plate"] == plate]:
            color = colors[r["type_id"] % len(colors)]
            x1 = int(round(r["x"] * px_per_mm))
            y1 = y0 + int(round(r["y"] * px_per_mm))
            x2 = x1 + int(round(r["w"] * px_per_mm))
            y2 = y1 + int(round(r["h"] * px_per_mm))
            draw.rectangle([x1, y1, x2, y2], fill=color, outline="#9a1515", width=1)
            label = item_names[r["type_id"]] if r["type_id"] < len(item_names) else str(r["type_id"])
            draw.text((x1 + 3, y1 + 3), label, fill="#111111", font=font)
    img.save(out_dir / "packingsolver_layout.png")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fixture",
        type=Path,
        default=REPO_ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json",
    )
    parser.add_argument("--out-dir", type=Path, default=repo_tmp("v63_packingsolver_benchmark"))
    parser.add_argument("--solver-exe", type=Path, default=default_solver_exe())
    parser.add_argument("--port", type=int, default=8113)
    parser.add_argument("--scale", type=int, default=10)
    parser.add_argument("--time-limit-s", type=float, default=2.0)
    parser.add_argument("--packing-mode", choices=["tree", "default"], default="tree")
    parser.add_argument("--lp-solver", default=None)
    parser.add_argument(
        "--repeat-factor",
        type=int,
        default=1,
        help="Multiply every item qty to synthesize larger ERP-like jobs.",
    )
    parser.add_argument("--skip-freecut", action="store_true")
    args = parser.parse_args()

    if not args.solver_exe.exists():
        raise FileNotFoundError(f"PackingSolver exe not found: {args.solver_exe}")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    req = repeat_fixture(load_fixture(args.fixture), args.repeat_factor)
    if args.repeat_factor > 1:
        (args.out_dir / f"generated_repeat_{args.repeat_factor}.json").write_text(
            json.dumps(req, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )
    input_meta = write_packingsolver_inputs(req, args.out_dir, args.scale)

    freecut_metrics = None
    if not args.skip_freecut:
        freecut_metrics = run_freecut_baselines(req, args.port, args.out_dir)

    pack_metrics = run_packingsolver(
        args.solver_exe,
        args.out_dir,
        args.time_limit_s,
        args.scale,
        req,
        args.packing_mode,
        args.lp_solver,
    )
    render_packingsolver_svg(req, args.out_dir, args.scale)

    summary = {
        "fixture": str(args.fixture),
        "repeat_factor": args.repeat_factor,
        "packing_mode": args.packing_mode,
        "input_meta": input_meta,
        "freecut_heuristic": freecut_metrics,
        "packingsolver_rectangleguillotine": pack_metrics,
    }
    (args.out_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    print(json.dumps(summary, indent=2, ensure_ascii=False))
    return 0 if pack_metrics.get("returncode") == 0 else int(pack_metrics.get("returncode", 1))


if __name__ == "__main__":
    sys.exit(main())
