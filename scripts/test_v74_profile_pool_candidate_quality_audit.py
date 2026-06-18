"""V74: audit individual profile_pool candidates by guarded group_shift quality.

This is a research harness. It does not call profile_pool directly. Instead it
runs each zone_penalty profile as a standalone request, collects the same
signals used by profile_pool ordering, and compares legacy ordering with the
V73 quality-aware ordering.
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
OUT_DIR = ROOT / "ai_docs" / "tmp" / "v74_profile_pool_candidate_quality_audit"
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
    zone_penalty: float,
    fill_penalty: float,
    time_limit_ms: int,
    restarts: int,
    include_svg: bool,
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
    params.pop("profile_pool", None)
    params["ga_override"] = {
        "zone_penalty": zone_penalty,
        "fill_penalty": fill_penalty,
    }
    if group_shift_enabled:
        params["group_shift"] = {
            "enabled": True,
            "min_shift_mm": group_shift_min_shift_mm,
            "max_passes": group_shift_max_passes,
            "debug_artifacts": include_svg,
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


def corner_free_rect_area(solution: dict[str, Any]) -> float:
    trim = solution.get("trim_mm") or {}
    width = float(solution["width_mm"]) - float(trim.get("left", 0.0)) - float(trim.get("right", 0.0))
    height = float(solution["height_mm"]) - float(trim.get("top", 0.0)) - float(trim.get("bottom", 0.0))
    placements = solution.get("placements", [])
    if not placements:
        return width * height
    lefts = [0.0]
    for placement in placements:
        right = float(placement["x_mm"]) + float(placement["width_mm"])
        if right < width:
            lefts.append(right)
    best = 0.0
    for left in lefts:
        max_bottom = 0.0
        for placement in placements:
            x = float(placement["x_mm"])
            right = x + float(placement["width_mm"])
            if x < width and right > left:
                max_bottom = max(max_bottom, float(placement["y_mm"]) + float(placement["height_mm"]))
        best = max(best, max(0.0, width - left) * max(0.0, height - max_bottom))
    return best


def summarize_candidate(
    seed: int,
    mode: str,
    zone_penalty: float,
    response: dict[str, Any],
    cut_gap_mm: float,
) -> dict[str, Any]:
    solutions = response.get("solutions", [])
    visual_by_sheet = [waste_regions_for_solution(solution, 0.0) for solution in solutions]
    cut_by_sheet = [waste_regions_for_solution(solution, cut_gap_mm) for solution in solutions]
    utils = [sheet_util_pct(solution) for solution in solutions]
    corners = [corner_free_rect_area(solution) for solution in solutions]
    summary = response.get("summary", {})
    group_shift = summary.get("group_shift") or {}
    return {
        "seed": seed,
        "mode": mode,
        "zone_penalty": zone_penalty,
        "sheets": len(solutions),
        "placements": sum(len(solution.get("placements", [])) for solution in solutions),
        "zones_visual0": sum(visual_by_sheet),
        "zones_cut_gap": sum(cut_by_sheet),
        "lead_util_pct": round(lead_util_pct(utils), 4),
        "max_corner_mm2": round(max(corners), 4) if corners else 0.0,
        "service_time_ms": summary.get("time_ms"),
        "group_shift_moves": group_shift.get("moves_applied", 0),
        "group_shift_quality_score_after": group_shift.get("quality_score_after", 0.0),
        "group_shift_quality_score_delta": group_shift.get("quality_score_delta", 0.0),
        "group_shift_topology_score_delta": group_shift.get("topology_score_delta", 0.0),
        "group_shift_part_contact_delta_mm": group_shift.get("part_contact_delta_mm", 0.0),
        "group_shift_contact_gain_mm": group_shift.get("contact_gain_mm", 0.0),
        "group_shift_after_mm2": group_shift.get("corridor_opportunity_after_mm2", 0.0),
        "group_shift_delta_mm2": group_shift.get("corridor_opportunity_delta_mm2", 0.0),
    }


def legacy_key(row: dict[str, Any]) -> tuple[Any, ...]:
    return (
        int(row["sheets"]),
        int(row["zones_visual0"]),
        int(row["zones_cut_gap"]),
        float(row["group_shift_after_mm2"]),
        -float(row["group_shift_contact_gain_mm"]),
        -float(row["group_shift_delta_mm2"]),
        -float(row["lead_util_pct"]),
        -float(row["max_corner_mm2"]),
    )


def quality_key(row: dict[str, Any]) -> tuple[Any, ...]:
    return (
        int(row["sheets"]),
        int(row["zones_visual0"]),
        int(row["zones_cut_gap"]),
        -float(row["group_shift_quality_score_after"]),
        -float(row["group_shift_quality_score_delta"]),
        -float(row["group_shift_topology_score_delta"]),
        -float(row["group_shift_part_contact_delta_mm"]),
        float(row["group_shift_after_mm2"]),
        -float(row["group_shift_contact_gain_mm"]),
        -float(row["group_shift_delta_mm2"]),
        -float(row["lead_util_pct"]),
        -float(row["max_corner_mm2"]),
    )


def rank_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[tuple[int, str], list[dict[str, Any]]] = {}
    for row in rows:
        groups.setdefault((int(row["seed"]), str(row["mode"])), []).append(row)

    ranked: list[dict[str, Any]] = []
    for key, group in groups.items():
        legacy_ordered = sorted(group, key=legacy_key)
        quality_ordered = sorted(group, key=quality_key)
        legacy_winner = legacy_ordered[0]
        quality_winner = quality_ordered[0]
        for idx, row in enumerate(legacy_ordered, start=1):
            row["legacy_rank"] = idx
        for idx, row in enumerate(quality_ordered, start=1):
            row["quality_rank"] = idx
            row["quality_changed_winner"] = quality_winner["zone_penalty"] != legacy_winner["zone_penalty"]
            row["legacy_winner_zone_penalty"] = legacy_winner["zone_penalty"]
            row["quality_winner_zone_penalty"] = quality_winner["zone_penalty"]
        ranked.extend(sorted(group, key=lambda item: float(item["zone_penalty"])))
    return sorted(ranked, key=lambda item: (int(item["seed"]), str(item["mode"]), float(item["zone_penalty"])))


def write_outputs(rows: list[dict[str, Any]], responses: dict[str, dict[str, Any]], include_svg: bool) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    if include_svg:
        for name, response in responses.items():
            artifacts = response.get("artifacts") or {}
            svg = artifacts.get("svg")
            if svg:
                (OUT_DIR / f"{name}.svg").write_text(svg, encoding="utf-8")

    fields = [
        "seed",
        "mode",
        "zone_penalty",
        "legacy_rank",
        "quality_rank",
        "quality_changed_winner",
        "legacy_winner_zone_penalty",
        "quality_winner_zone_penalty",
        "sheets",
        "placements",
        "zones_visual0",
        "zones_cut_gap",
        "lead_util_pct",
        "max_corner_mm2",
        "group_shift_quality_score_after",
        "group_shift_quality_score_delta",
        "group_shift_topology_score_delta",
        "group_shift_part_contact_delta_mm",
        "group_shift_contact_gain_mm",
        "group_shift_after_mm2",
        "group_shift_delta_mm2",
        "group_shift_moves",
        "service_time_ms",
    ]
    with (OUT_DIR / "v74_candidate_metrics.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field) for field in fields})
    (OUT_DIR / "v74_candidate_metrics.json").write_text(
        json.dumps({"rows": rows}, indent=2),
        encoding="utf-8",
    )

    groups: dict[tuple[int, str], list[dict[str, Any]]] = {}
    for row in rows:
        groups.setdefault((int(row["seed"]), str(row["mode"])), []).append(row)
    changed = [
        group[0]
        for group in groups.values()
        if any(row.get("quality_changed_winner") for row in group)
    ]
    lines = [
        "# V74 profile_pool candidate quality audit",
        "",
        f"candidate groups: {len(groups)}",
        f"groups where quality winner differs from legacy winner: {len(changed)}",
        "",
        "| seed | mode | legacy zp | quality zp | changed | legacy visual/cut | quality visual/cut | legacy q after | quality q after |",
        "|---:|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for key in sorted(groups):
        group = groups[key]
        legacy = min(group, key=legacy_key)
        quality = min(group, key=quality_key)
        lines.append(
            f"| {key[0]} | {key[1]} | {legacy['zone_penalty']} | {quality['zone_penalty']} | "
            f"{quality['zone_penalty'] != legacy['zone_penalty']} | "
            f"{legacy['zones_visual0']}/{legacy['zones_cut_gap']} | "
            f"{quality['zones_visual0']}/{quality['zones_cut_gap']} | "
            f"{legacy['group_shift_quality_score_after']} | "
            f"{quality['group_shift_quality_score_after']} |"
        )
    (OUT_DIR / "v74_summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


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
                for zone_penalty in args.zone_penalties:
                    request = make_request(
                        seed,
                        zone_penalty,
                        args.fill_penalty,
                        args.time_limit_ms,
                        args.restarts,
                        args.include_svg,
                        group_shift_enabled,
                        args.group_shift_min_shift_mm,
                        args.group_shift_max_passes,
                    )
                    response = post_json(base_url, request, args.request_timeout_s)
                    name = f"v74_seed_{seed}_{mode}_zp_{zone_penalty}".replace(".", "p")
                    responses[name] = response
                    rows.append(
                        summarize_candidate(
                            seed,
                            mode,
                            zone_penalty,
                            response,
                            args.cut_gap_mm,
                        )
                    )
        write_outputs(rank_rows(rows), responses, args.include_svg)
        print(f"wrote {len(rows)} candidate rows to {OUT_DIR}")
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
    parser.add_argument("--port", type=int, default=8137)
    parser.add_argument("--seeds", type=int, nargs="+", default=[1, 2, 3, 4, 5, 6])
    parser.add_argument("--zone-penalties", type=float, nargs="+", default=[0.2, 0.3, 0.4, 0.5, 0.6, 0.8])
    parser.add_argument("--fill-penalty", type=float, default=0.1)
    parser.add_argument("--time-limit-ms", type=int, default=3000)
    parser.add_argument("--restarts", type=int, default=3)
    parser.add_argument("--group-shift-min-shift-mm", type=float, default=5.0)
    parser.add_argument("--group-shift-max-passes", type=int, default=4)
    parser.add_argument("--cut-gap-mm", type=float, default=6.5)
    parser.add_argument("--request-timeout-s", type=float, default=180.0)
    parser.add_argument("--startup-timeout-s", type=float, default=120.0)
    parser.add_argument("--include-svg", action=argparse.BooleanOptionalAction, default=False)
    parser.add_argument("--out-dir")
    return parser.parse_args()


if __name__ == "__main__":
    try:
        run_benchmark(parse_args())
    except Exception as exc:  # noqa: BLE001 - CLI diagnostic
        print(f"ERROR: {exc}", file=sys.stderr)
        raise
