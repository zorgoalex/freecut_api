#!/usr/bin/env python3
import argparse
import copy
import json
import os
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import requests
from cairosvg import svg2png
from PIL import Image, ImageDraw

from optimize_search import calculate_internal_void_metrics


def load_json(path: str) -> Dict[str, Any]:
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def count_placeable_unplaced(unplaced_items: List[Dict[str, Any]]) -> int:
    return sum(1 for it in unplaced_items if it.get("reason") != "oversized")


def count_placed_instances(solutions: List[Dict[str, Any]]) -> int:
    return sum(len(sol.get("placements", [])) for sol in solutions)


def compute_placeable_ratio(body: Dict[str, Any]) -> float:
    solutions = body.get("solutions", [])
    unplaced = body.get("unplaced_items", [])
    placed = count_placed_instances(solutions)
    placeable_unplaced = count_placeable_unplaced(unplaced)
    total_placeable = placed + placeable_unplaced
    if total_placeable == 0:
        return 1.0
    return placed / total_placeable


def compute_quality_metrics(
    body: Dict[str, Any],
    grid_mm: float,
    spacing_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
    solutions = body.get("solutions", [])
    (
        internal_void,
        internal_components,
        exposure_penalty,
        corridor_void,
        corridor_components,
        row_gap,
        col_gap,
        corridor_weighted,
        max_penetration,
        penetration_volume,
        penetration_weighted,
        occupied_perimeter,
        void_compactness,
        edge_continuity,
        edge_breaks,
        corners_filled,
    ) = calculate_internal_void_metrics(
        solutions=solutions,
        grid_mm=grid_mm,
        pad_mm=0.0,
        spacing_mm=spacing_mm,
        corridor_ok_mult=corridor_ok_mult,
    )

    waste = body.get("summary", {}).get("waste_percent")
    if waste is None:
        waste = float("inf")

    placeable_ratio = compute_placeable_ratio(body)
    hard_ok = (placeable_ratio == 1.0) and (float(internal_void) == 0.0)
    sort_key = (
        0 if hard_ok else 1,
        float(exposure_penalty),
        float(penetration_weighted),
        float(corridor_void),
        float(occupied_perimeter),
        int(corridor_components),
        int(edge_breaks),
        float(internal_void),
        float(waste),
    )
    return {
        "placeable_ratio": placeable_ratio,
        "internal_void": float(internal_void),
        "internal_components": int(internal_components),
        "exposure_penalty": float(exposure_penalty),
        "occupied_perimeter": float(occupied_perimeter),
        "void_compactness": float(void_compactness),
        "corridor_components": int(corridor_components),
        "corridor_void": float(corridor_void),
        "corridor_weighted": float(corridor_weighted),
        "row_gap": float(row_gap),
        "col_gap": float(col_gap),
        "max_penetration": float(max_penetration),
        "penetration_volume": float(penetration_volume),
        "penetration_weighted": float(penetration_weighted),
        "edge_continuity": float(edge_continuity),
        "edge_breaks": int(edge_breaks),
        "corners_filled": int(corners_filled),
        "waste_percent": float(waste),
        "hard_ok": hard_ok,
        "sort_key": sort_key,
    }


def build_mode_payload(
    template: Dict[str, Any],
    mode: str,
    seed: Optional[int],
    mode_time_limits: Dict[str, int],
    restarts: int,
) -> Tuple[str, Dict[str, Any], Dict[str, Any]]:
    payload = copy.deepcopy(template)
    params = payload.setdefault("params", {})

    # Shared defaults from "best params" tests.
    params["include_svg"] = True
    if seed is None:
        params.pop("seed", None)
    else:
        params["seed"] = seed
    params["layout_mode"] = "guillotine"
    params["restarts"] = int(restarts)
    params.pop("portfolio", None)
    params.pop("beam", None)
    params.pop("alns", None)

    if mode == "standard":
        endpoint = "/v1/optimize"
        params["time_limit_ms"] = int(mode_time_limits["standard"])
        params["sla_profile"] = "balanced"
    elif mode == "portfolio":
        endpoint = "/v1/optimize"
        params["time_limit_ms"] = int(mode_time_limits["portfolio"])
        params["portfolio"] = {
            "enabled": True,
            "deadline_ms": int(mode_time_limits["portfolio"]),
            "candidate_count": 2,
        }
    elif mode == "beam":
        endpoint = "/v1/optimize/beam"
        params["time_limit_ms"] = int(mode_time_limits["beam"])
        params["beam"] = {
            "enabled": True,
            "deadline_ms": int(mode_time_limits["beam"]),
            "beam_width": 2,
            "beam_depth": 2,
            "branch_factor": 2,
        }
    elif mode == "alns":
        endpoint = "/v1/optimize/alns"
        params["time_limit_ms"] = int(mode_time_limits["alns"])
        params["alns"] = {
            "enabled": True,
            "deadline_ms": int(mode_time_limits["alns"]),
            "iterations": 24,
            "segment_size": 6,
            "temperature_start": 1.0,
            "temperature_end": 0.12,
            "reaction_factor": 0.3,
        }
    else:
        raise ValueError(f"unsupported mode: {mode}")

    return endpoint, payload, params


def safe_clear_dir(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    for child in path.iterdir():
        if child.is_file() and (child.suffix.lower() in {".svg", ".png", ".json"}):
            child.unlink()


def save_overview_png(items: List[Dict[str, Any]], out_file: Path) -> None:
    if not items:
        return
    thumbs: List[Tuple[Image.Image, str]] = []
    for it in items:
        svg_path = Path(it["svg_path"])
        png_bytes = svg2png(url=str(svg_path))
        with Image.open(io_bytes(png_bytes)) as img:
            img = img.convert("RGB")
            img.thumbnail((620, 420))
            shown_seed = it.get("used_seed", it.get("seed"))
            label = (
                f"#{it['rank']} seed={shown_seed} "
                f"waste={it['waste_percent']:.2f} vcomp={it['void_compactness']:.3f}"
            )
            thumbs.append((img.copy(), label))

    cols = 2
    rows = (len(thumbs) + cols - 1) // cols
    card_w, card_h = 640, 470
    sheet = Image.new("RGB", (cols * card_w, rows * card_h), (250, 250, 250))
    draw = ImageDraw.Draw(sheet)

    for idx, (img, label) in enumerate(thumbs):
        r = idx // cols
        c = idx % cols
        x0 = c * card_w
        y0 = r * card_h
        sheet.paste(img, (x0 + 10, y0 + 10))
        draw.text((x0 + 10, y0 + 435), label, fill=(30, 30, 30))

    sheet.save(out_file)


def io_bytes(data: bytes):
    import io

    return io.BytesIO(data)


def run_mode(
    base_url: str,
    mode: str,
    template: Dict[str, Any],
    seeds: List[int],
    top_n: int,
    grid_mm: float,
    corridor_ok_mult: float,
    out_root: Path,
    omit_seed: bool,
    mode_time_limits: Dict[str, int],
    restarts: int,
) -> Dict[str, Any]:
    out_dir = out_root / mode
    safe_clear_dir(out_dir)

    records: List[Dict[str, Any]] = []
    status_counts: Dict[str, int] = {}

    for idx, seed in enumerate(seeds):
        if idx > 0 and idx % 10 == 0:
            print(f"[progress] mode={mode} run={idx}/{len(seeds)}")
        request_seed = None if omit_seed else seed
        endpoint, payload, params = build_mode_payload(
            template,
            mode,
            request_seed,
            mode_time_limits,
            restarts,
        )
        timeout_s = max(12.0, float(params["time_limit_ms"]) / 1000.0 + 10.0)
        resp = requests.post(
            f"{base_url}{endpoint}",
            json=payload,
            timeout=timeout_s,
            headers={"Content-Type": "application/json"},
        )
        status_counts[str(resp.status_code)] = status_counts.get(str(resp.status_code), 0) + 1

        rec: Dict[str, Any] = {
            "mode": mode,
            "seed": request_seed,
            "endpoint": endpoint,
            "status": resp.status_code,
            "params": {
                "time_limit_ms": params.get("time_limit_ms"),
                "restarts": params.get("restarts"),
                "sla_profile": params.get("sla_profile"),
                "portfolio": params.get("portfolio"),
                "beam": params.get("beam"),
                "alns": params.get("alns"),
            },
        }

        if resp.status_code == 200:
            body = resp.json()
            q = compute_quality_metrics(
                body=body,
                grid_mm=grid_mm,
                spacing_mm=float(payload.get("params", {}).get("spacing_mm", 0.0)),
                corridor_ok_mult=corridor_ok_mult,
            )
            summary = body.get("summary", {})
            rec.update(q)
            rec["time_ms"] = summary.get("time_ms")
            rec["used_seed"] = summary.get("used_seed")
            rec["timeout_reason"] = summary.get("timeout_reason")
            rec["svg"] = body.get("artifacts", {}).get("svg", "")
        records.append(rec)

    ok_records = [r for r in records if r.get("status") == 200 and r.get("svg")]
    hard_ok = [r for r in ok_records if r.get("hard_ok")]
    hard_ok.sort(key=lambda x: tuple(x["sort_key"]))

    selected = hard_ok[:top_n]
    if len(selected) < top_n:
        fallback = [r for r in ok_records if r not in selected]
        fallback.sort(key=lambda x: tuple(x.get("sort_key", (float("inf"),))))
        selected.extend(fallback[: top_n - len(selected)])

    manifest_items = []
    for rank, item in enumerate(selected, start=1):
        exp_m = int(round(float(item.get("exposure_penalty", 0.0)) / 1e6))
        pen_g = int(round(float(item.get("penetration_weighted", 0.0)) / 1e9))
        svg_name = (
            f"rank_{rank:02d}_seed_{item.get('used_seed', item.get('seed'))}"
            f"_expM_{exp_m}"
            f"_penG_{pen_g}"
            f"_waste_{item.get('waste_percent', 0.0):.2f}.svg"
        )
        svg_path = out_dir / svg_name
        with open(svg_path, "w", encoding="utf-8") as f:
            f.write(item.get("svg", ""))

        m = {
            "rank": rank,
            "seed": item["seed"],
            "used_seed": item.get("used_seed"),
            "status": item["status"],
            "time_ms": item.get("time_ms"),
            "timeout_reason": item.get("timeout_reason"),
            "hard_ok": item.get("hard_ok"),
            "placeable_ratio": item.get("placeable_ratio"),
            "internal_void": item.get("internal_void"),
            "exposure_penalty": item.get("exposure_penalty"),
            "penetration_weighted": item.get("penetration_weighted"),
            "occupied_perimeter": item.get("occupied_perimeter"),
            "void_compactness": item.get("void_compactness"),
            "corridor_components": item.get("corridor_components"),
            "edge_breaks": item.get("edge_breaks"),
            "waste_percent": item.get("waste_percent"),
            "sort_key": item.get("sort_key"),
            "svg_path": str(svg_path),
        }
        manifest_items.append(m)

    manifest = {
        "mode": mode,
        "status_counts": status_counts,
        "total_runs": len(seeds),
        "ok_runs": len(ok_records),
        "hard_ok_runs": len(hard_ok),
        "selected_top_n": len(manifest_items),
        "items": manifest_items,
    }
    with open(out_dir / "manifest.json", "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)

    if manifest_items:
        save_overview_png(manifest_items, out_dir / "overview.png")

    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description="Find top-N layouts per algorithm and save SVG + visual overviews.")
    parser.add_argument("--base-url", default="http://127.0.0.1:8088")
    parser.add_argument("--fixture", default="tests/fixtures/multisheet_oversized.json")
    parser.add_argument("--seed-start", type=int, default=1)
    parser.add_argument("--seeds", type=int, default=60)
    parser.add_argument("--top-n", type=int, default=10)
    parser.add_argument("--grid-mm", type=float, default=5.0)
    parser.add_argument("--corridor-ok-mult", type=float, default=3.0)
    parser.add_argument("--out-dir", default="ai_docs/candidate_layouts/top10_algorithms")
    parser.add_argument("--restarts", type=int, default=2)
    parser.add_argument("--standard-time-limit-ms", type=int, default=2000)
    parser.add_argument("--portfolio-time-limit-ms", type=int, default=3000)
    parser.add_argument("--beam-time-limit-ms", type=int, default=4900)
    parser.add_argument("--alns-time-limit-ms", type=int, default=1250)
    parser.add_argument(
        "--omit-seed",
        action="store_true",
        help="Do not pass params.seed; let server generate random seed per request.",
    )
    args = parser.parse_args()

    template = load_json(args.fixture)
    seeds = list(range(args.seed_start, args.seed_start + args.seeds))
    mode_time_limits = {
        "standard": int(args.standard_time_limit_ms),
        "portfolio": int(args.portfolio_time_limit_ms),
        "beam": int(args.beam_time_limit_ms),
        "alns": int(args.alns_time_limit_ms),
    }
    out_root = Path(args.out_dir)
    out_root.mkdir(parents=True, exist_ok=True)

    report = {
        "fixture": args.fixture,
        "seeds": {"start": args.seed_start, "count": args.seeds},
        "seed_mode": "server_default_random" if args.omit_seed else "explicit_sequence",
        "mode_time_limits_ms": mode_time_limits,
        "restarts": int(args.restarts),
        "top_n": args.top_n,
        "formula": [
            "internal_void",
            "occupied_perimeter",
            "void_compactness",
            "corridor_components",
            "waste_percent",
        ],
        "hard_constraints": [
            "placeable_placed_ratio == 1.0",
            "internal_void == 0",
        ],
        "modes": {},
    }

    for mode in ["standard", "portfolio", "beam", "alns"]:
        print(f"[run] mode={mode}")
        manifest = run_mode(
            base_url=args.base_url,
            mode=mode,
            template=template,
            seeds=seeds,
            top_n=args.top_n,
            grid_mm=args.grid_mm,
            corridor_ok_mult=args.corridor_ok_mult,
            out_root=out_root,
            omit_seed=args.omit_seed,
            mode_time_limits=mode_time_limits,
            restarts=int(args.restarts),
        )
        report["modes"][mode] = {
            "status_counts": manifest["status_counts"],
            "ok_runs": manifest["ok_runs"],
            "hard_ok_runs": manifest["hard_ok_runs"],
            "selected_top_n": manifest["selected_top_n"],
            "manifest_path": str((out_root / mode / "manifest.json")),
            "overview_png": str((out_root / mode / "overview.png")),
        }
        print(
            f"[done] mode={mode} ok={manifest['ok_runs']}/{manifest['total_runs']} "
            f"hard_ok={manifest['hard_ok_runs']} selected={manifest['selected_top_n']}"
        )

    report_path = out_root / "report.json"
    with open(report_path, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print(f"[done] report saved: {report_path}")


if __name__ == "__main__":
    main()
