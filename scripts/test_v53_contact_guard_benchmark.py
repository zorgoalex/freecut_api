"""V53/V73: benchmark group_shift-aware profile_pool scoring.

This is a research harness, not a production path. It starts the current
Freecut service, runs paired profile_pool requests with seed-offset rescue,
saves JSON/SVG artifacts, and reports visual zones, cut-gap zones, and
group_shift contact/quality telemetry.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections import deque
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "multisheet_varied_4sheets.json"
OUT_DIR = ROOT / "ai_docs" / "tmp" / "v53_contact_guard_benchmark"
CELL_MM = 10.0
MIN_REGION_CELLS = 50


def wait_ready(base_url: str, timeout_s: float) -> None:
    deadline = time.time() + timeout_s
    last_error: Exception | None = None
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/health/ready", timeout=2) as response:
                if response.status == 200:
                    return
        except Exception as exc:  # noqa: BLE001 - diagnostic retry loop
            last_error = exc
        time.sleep(0.5)
    raise RuntimeError(f"server not ready after {timeout_s}s: {last_error}")


def start_server(port: int) -> subprocess.Popen[str]:
    env = os.environ.copy()
    env["PORT"] = str(port)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = (OUT_DIR / "server.out.log").open("w", encoding="utf-8")
    err = (OUT_DIR / "server.err.log").open("w", encoding="utf-8")
    return subprocess.Popen(
        ["cargo", "run", "--release"],
        cwd=ROOT,
        env=env,
        stdout=out,
        stderr=err,
        text=True,
    )


def post_json(base_url: str, payload: dict[str, Any], timeout_s: float) -> dict[str, Any]:
    body = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        f"{base_url}/v1/optimize",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        error_body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {error_body}") from exc


def make_request(
    seed: int,
    time_limit_ms: int,
    restarts: int,
    include_svg: bool,
    seed_offsets: list[int],
    group_shift_enabled: bool,
    group_shift_min_shift_mm: float,
    group_shift_max_passes: int,
) -> dict[str, Any]:
    request = json.loads(FIXTURE.read_text(encoding="utf-8"))
    params = request["params"]
    params["seed"] = seed
    params["time_limit_ms"] = time_limit_ms
    params["restarts"] = restarts
    params["retry_strategy"] = "disabled"
    params["include_svg"] = include_svg
    params["profile_pool"] = {
        "enabled": True,
        "zone_penalties": [0.2, 0.3, 0.4, 0.5, 0.6, 0.8],
        "fill_penalty": 0.1,
        "max_lead_drop_pp": 0.8,
    }
    if seed_offsets:
        params["profile_pool"]["seed_offsets"] = seed_offsets
    if group_shift_enabled:
        params["group_shift"] = {
            "enabled": True,
            "min_shift_mm": group_shift_min_shift_mm,
            "max_passes": group_shift_max_passes,
            "debug_artifacts": True,
        }
    else:
        params.pop("group_shift", None)
    return request


def waste_regions_for_solution(solution: dict[str, Any], gap_mm: float) -> int:
    trim = solution.get("trim_mm") or {}
    width = float(solution["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    height = float(solution["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    nx = int(width // CELL_MM)
    ny = int(height // CELL_MM)
    if nx <= 0 or ny <= 0:
        return 0

    occ = [[False for _ in range(nx)] for _ in range(ny)]
    for placement in solution.get("placements", []):
        x0 = max(0.0, float(placement["x_mm"]) - gap_mm)
        y0 = max(0.0, float(placement["y_mm"]) - gap_mm)
        x1 = min(width, float(placement["x_mm"]) + float(placement["width_mm"]) + gap_mm)
        y1 = min(height, float(placement["y_mm"]) + float(placement["height_mm"]) + gap_mm)
        if x1 <= x0 or y1 <= y0:
            continue
        i0 = int(x0 // CELL_MM)
        j0 = int(y0 // CELL_MM)
        i1 = min(nx - 1, int((x1 - 1e-9) // CELL_MM))
        j1 = min(ny - 1, int((y1 - 1e-9) // CELL_MM))
        for j in range(j0, j1 + 1):
            for i in range(i0, i1 + 1):
                occ[j][i] = True

    seen = [[False for _ in range(nx)] for _ in range(ny)]
    regions = 0
    for start_j in range(ny):
        for start_i in range(nx):
            if occ[start_j][start_i] or seen[start_j][start_i]:
                continue
            cells = 0
            queue: deque[tuple[int, int]] = deque([(start_i, start_j)])
            seen[start_j][start_i] = True
            while queue:
                i, j = queue.pop()
                cells += 1
                for ni, nj in ((i - 1, j), (i + 1, j), (i, j - 1), (i, j + 1)):
                    if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                        seen[nj][ni] = True
                        queue.append((ni, nj))
            if cells >= MIN_REGION_CELLS:
                regions += 1
    return regions


def sheet_util_pct(solution: dict[str, Any]) -> float:
    trim = solution.get("trim_mm") or {}
    width = float(solution["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    height = float(solution["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    area = width * height
    if area <= 0:
        return 0.0
    used = sum(float(p["width_mm"]) * float(p["height_mm"]) for p in solution.get("placements", []))
    return used / area * 100.0


def lead_util_pct(utils: list[float]) -> float:
    if not utils:
        return 0.0
    ordered = sorted(utils, reverse=True)
    if len(ordered) == 1:
        return ordered[0]
    return sum(ordered[:-1]) / (len(ordered) - 1)


def summarize_response(name: str, response: dict[str, Any], cut_gap_mm: float) -> dict[str, Any]:
    solutions = response.get("solutions", [])
    visual_by_sheet = [waste_regions_for_solution(solution, 0.0) for solution in solutions]
    cut_by_sheet = [waste_regions_for_solution(solution, cut_gap_mm) for solution in solutions]
    utils = [sheet_util_pct(solution) for solution in solutions]
    summary = response.get("summary", {})
    pool = summary.get("profile_pool") or {}
    group_shift = summary.get("group_shift") or {}
    return {
        "case": name,
        "sheets": len(solutions),
        "placements": sum(len(solution.get("placements", [])) for solution in solutions),
        "zones_visual0": sum(visual_by_sheet),
        "zones_cut_gap": sum(cut_by_sheet),
        "per_sheet_visual0": visual_by_sheet,
        "per_sheet_cut_gap": cut_by_sheet,
        "lead_util_pct": round(lead_util_pct(utils), 4),
        "min_util_pct": round(min(utils), 4) if utils else 0.0,
        "service_time_ms": summary.get("time_ms"),
        "winner_zone_penalty": pool.get("winner_zone_penalty"),
        "winner_visual_waste_regions": pool.get("winner_visual_waste_regions"),
        "winner_waste_regions": pool.get("winner_waste_regions"),
        "winner_lead_util_pct": pool.get("winner_lead_util_pct"),
        "winner_group_shift_contact_gain_mm": pool.get("winner_group_shift_contact_gain_mm"),
        "winner_group_shift_quality_score_after": pool.get("winner_group_shift_quality_score_after"),
        "winner_group_shift_quality_score_delta": pool.get("winner_group_shift_quality_score_delta"),
        "winner_group_shift_topology_score_delta": pool.get("winner_group_shift_topology_score_delta"),
        "winner_group_shift_part_contact_delta_mm": pool.get("winner_group_shift_part_contact_delta_mm"),
        "winner_group_shift_after_mm2": pool.get("winner_group_shift_opportunity_after_mm2"),
        "winner_group_shift_delta_mm2": pool.get("winner_group_shift_opportunity_delta_mm2"),
        "quality_scoring_changed_winner": pool.get("quality_scoring_changed_winner"),
        "legacy_winner_seed": pool.get("legacy_winner_seed"),
        "legacy_winner_zone_penalty": pool.get("legacy_winner_zone_penalty"),
        "legacy_winner_group_shift_quality_score_after": pool.get(
            "legacy_winner_group_shift_quality_score_after"
        ),
        "legacy_winner_group_shift_quality_score_delta": pool.get(
            "legacy_winner_group_shift_quality_score_delta"
        ),
        "rescue_triggered": pool.get("rescue_triggered"),
        "seed_offsets_used": pool.get("seed_offsets_used"),
        "candidates_completed": pool.get("candidates_completed"),
        "group_shift_moves": group_shift.get("moves_applied"),
        "group_shift_parts_moved": group_shift.get("parts_moved"),
        "group_shift_contact_gain_mm": group_shift.get("contact_gain_mm"),
        "group_shift_quality_score_after": group_shift.get("quality_score_after"),
        "group_shift_quality_score_delta": group_shift.get("quality_score_delta"),
        "group_shift_topology_score_delta": group_shift.get("topology_score_delta"),
        "group_shift_part_contact_delta_mm": group_shift.get("part_contact_delta_mm"),
        "group_shift_closed_area_mm2": group_shift.get("corridor_closed_area_mm2"),
        "group_shift_time_ms": group_shift.get("time_ms"),
    }


def write_outputs(rows: list[dict[str, Any]], responses: dict[str, dict[str, Any]], cut_gap_mm: float) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, response in responses.items():
        (OUT_DIR / f"{name}.json").write_text(json.dumps(response, indent=2), encoding="utf-8")
        artifacts = response.get("artifacts") or {}
        artifact_names = {
            "svg": f"{name}.svg",
            "group_shift_before_svg": f"{name}_group_shift_before.svg",
            "group_shift_diff_svg": f"{name}_group_shift_diff.svg",
        }
        for key, file_name in artifact_names.items():
            svg = artifacts.get(key)
            if svg:
                (OUT_DIR / file_name).write_text(svg, encoding="utf-8")

    (OUT_DIR / "v53_metrics.json").write_text(
        json.dumps({"cut_gap_mm": cut_gap_mm, "rows": rows}, indent=2),
        encoding="utf-8",
    )
    fields = [
        "case",
        "sheets",
        "placements",
        "zones_visual0",
        "zones_cut_gap",
        "lead_util_pct",
        "min_util_pct",
        "service_time_ms",
        "winner_zone_penalty",
        "winner_visual_waste_regions",
        "winner_waste_regions",
        "winner_lead_util_pct",
        "winner_group_shift_contact_gain_mm",
        "winner_group_shift_quality_score_after",
        "winner_group_shift_quality_score_delta",
        "winner_group_shift_topology_score_delta",
        "winner_group_shift_part_contact_delta_mm",
        "winner_group_shift_after_mm2",
        "winner_group_shift_delta_mm2",
        "quality_scoring_changed_winner",
        "legacy_winner_seed",
        "legacy_winner_zone_penalty",
        "legacy_winner_group_shift_quality_score_after",
        "legacy_winner_group_shift_quality_score_delta",
        "rescue_triggered",
        "seed_offsets_used",
        "candidates_completed",
        "group_shift_moves",
        "group_shift_parts_moved",
        "group_shift_contact_gain_mm",
        "group_shift_quality_score_after",
        "group_shift_quality_score_delta",
        "group_shift_topology_score_delta",
        "group_shift_part_contact_delta_mm",
        "group_shift_closed_area_mm2",
        "group_shift_time_ms",
    ]
    with (OUT_DIR / "v53_metrics.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field) for field in fields})

    lines = [
        "# V53/V73 group_shift-aware profile_pool benchmark",
        "",
        f"cut_gap_mm: {cut_gap_mm}",
        "",
        "| case | sheets | visual0 | cut_gap | lead % | zp | changed | legacy zp | winner q after | legacy q after | winner q delta | winner contact | moves | actual q delta | actual contact | time ms | completed |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row['case']} | {row['sheets']} | {row['zones_visual0']} | {row['zones_cut_gap']} | "
            f"{row['lead_util_pct']} | {row['winner_zone_penalty']} | "
            f"{row['quality_scoring_changed_winner']} | "
            f"{row['legacy_winner_zone_penalty']} | "
            f"{row['winner_group_shift_quality_score_after']} | "
            f"{row['legacy_winner_group_shift_quality_score_after']} | "
            f"{row['winner_group_shift_quality_score_delta']} | "
            f"{row['winner_group_shift_contact_gain_mm']} | {row['group_shift_moves']} | "
            f"{row['group_shift_quality_score_delta']} | "
            f"{row['group_shift_contact_gain_mm']} | {row['service_time_ms']} | "
            f"{row['candidates_completed']} |"
        )
    lines.append("")
    lines.append(
        "Interpretation: compare *_off vs *_on for the same seed. The *_on rows include "
        "group_shift and expose winner/actual contact and quality deltas, so V53/V73 "
        "can be judged against visual zones, cut-gap zones, and local anchor compaction."
    )
    (OUT_DIR / "v53_summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_benchmark(args: argparse.Namespace) -> None:
    global OUT_DIR
    if args.out_dir:
        OUT_DIR = Path(args.out_dir)
    base_url = args.server_url or f"http://127.0.0.1:{args.port}"
    server: subprocess.Popen[str] | None = None
    try:
        if args.server_url is None:
            server = start_server(args.port)
            wait_ready(base_url, args.startup_timeout_s)
        rows: list[dict[str, Any]] = []
        responses: dict[str, dict[str, Any]] = {}
        for seed in args.seeds:
            for mode, group_shift_enabled in (("off", False), ("on", True)):
                name = f"v53_seed_{seed}_{mode}"
                request = make_request(
                    seed,
                    args.time_limit_ms,
                    args.restarts,
                    args.include_svg,
                    args.seed_offsets,
                    group_shift_enabled,
                    args.group_shift_min_shift_mm,
                    args.group_shift_max_passes,
                )
                response = post_json(base_url, request, args.request_timeout_s)
                responses[name] = response
                rows.append(summarize_response(name, response, args.cut_gap_mm))
        write_outputs(rows, responses, args.cut_gap_mm)
        print(f"wrote {len(rows)} rows to {OUT_DIR}")
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--server-url")
    parser.add_argument("--port", type=int, default=8101)
    parser.add_argument("--seeds", type=int, nargs="+", default=[11, 13])
    parser.add_argument("--time-limit-ms", type=int, default=4000)
    parser.add_argument("--restarts", type=int, default=5)
    parser.add_argument("--seed-offsets", type=int, nargs="*", default=[1, 2, 3, 5, 7, 8, 13, 21])
    parser.add_argument("--group-shift-min-shift-mm", type=float, default=5.0)
    parser.add_argument("--group-shift-max-passes", type=int, default=4)
    parser.add_argument("--cut-gap-mm", type=float, default=6.5)
    parser.add_argument("--out-dir")
    parser.add_argument("--request-timeout-s", type=float, default=240.0)
    parser.add_argument("--startup-timeout-s", type=float, default=120.0)
    parser.add_argument("--include-svg", action=argparse.BooleanOptionalAction, default=True)
    return parser.parse_args()


if __name__ == "__main__":
    try:
        run_benchmark(parse_args())
    except Exception as exc:  # noqa: BLE001 - CLI diagnostic
        print(f"ERROR: {exc}", file=sys.stderr)
        raise
