#!/usr/bin/env python3
import argparse
import copy
import json
import statistics
import time
from typing import Any, Dict, List, Optional, Tuple

import requests

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
        _internal_components,
        _exposure_penalty,
        _corridor_void,
        corridor_components,
        _row_gap,
        _col_gap,
        _corridor_weighted,
        _max_penetration,
        _penetration_volume,
        _penetration_weighted,
        occupied_perimeter,
        void_compactness,
        _edge_continuity,
        _edge_breaks,
        _corners_filled,
    ) = calculate_internal_void_metrics(
        solutions=solutions,
        grid_mm=grid_mm,
        pad_mm=0.0,
        spacing_mm=spacing_mm,
        corridor_ok_mult=corridor_ok_mult,
    )

    waste = (
        body.get("summary", {}).get("waste_percent")
        if isinstance(body.get("summary"), dict)
        else None
    )
    if waste is None:
        waste = float("inf")

    placeable_ratio = compute_placeable_ratio(body)
    hard_ok = (placeable_ratio == 1.0) and (internal_void == 0.0)
    sort_key = (
        float(internal_void),
        float(occupied_perimeter),
        float(void_compactness),
        int(corridor_components),
        float(waste),
    )
    return {
        "placeable_ratio": placeable_ratio,
        "internal_void": float(internal_void),
        "occupied_perimeter": float(occupied_perimeter),
        "void_compactness": float(void_compactness),
        "corridor_components": int(corridor_components),
        "waste_percent": float(waste),
        "hard_ok": hard_ok,
        "sort_key": sort_key,
    }


def post_optimize(base_url: str, endpoint: str, payload: Dict[str, Any], timeout_s: float) -> Tuple[int, Dict[str, Any], float]:
    t0 = time.time()
    resp = requests.post(
        f"{base_url}{endpoint}",
        json=payload,
        timeout=timeout_s,
        headers={"Content-Type": "application/json"},
    )
    elapsed_ms = (time.time() - t0) * 1000.0
    try:
        body = resp.json()
    except Exception:
        body = {}
    return resp.status_code, body, elapsed_ms


def make_payload(
    template: Dict[str, Any],
    seed: int,
    time_limit_ms: int,
    restarts: int,
    include_svg: bool,
    beam: bool,
) -> Dict[str, Any]:
    payload = copy.deepcopy(template)
    params = payload.setdefault("params", {})
    params["seed"] = seed
    params["time_limit_ms"] = time_limit_ms
    params["restarts"] = restarts
    params["include_svg"] = include_svg
    params["layout_mode"] = "guillotine"
    if beam:
        params["beam"] = {
            "enabled": True,
            "deadline_ms": time_limit_ms,
            "beam_width": 2,
            "beam_depth": 2,
            "branch_factor": 2,
        }
        params.pop("portfolio", None)
    else:
        params.pop("beam", None)
        params.pop("portfolio", None)
    return payload


def run_series(
    base_url: str,
    endpoint: str,
    template: Dict[str, Any],
    seeds: List[int],
    time_limit_ms: int,
    restarts: int,
    grid_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
    timeout_s = max(10.0, (time_limit_ms / 1000.0) + 5.0)
    records = []
    ok = 0
    hard_ok = 0
    latencies: List[float] = []

    for seed in seeds:
        payload = make_payload(
            template=template,
            seed=seed,
            time_limit_ms=time_limit_ms,
            restarts=restarts,
            include_svg=False,
            beam=(endpoint == "/v1/optimize/beam"),
        )
        status, body, elapsed_ms = post_optimize(base_url, endpoint, payload, timeout_s=timeout_s)
        row: Dict[str, Any] = {
            "seed": seed,
            "status": status,
            "elapsed_ms": elapsed_ms,
        }
        if status == 200:
            ok += 1
            latencies.append(elapsed_ms)
            spacing = float(payload.get("params", {}).get("spacing_mm", 0.0))
            q = compute_quality_metrics(
                body=body,
                grid_mm=grid_mm,
                spacing_mm=spacing,
                corridor_ok_mult=corridor_ok_mult,
            )
            row.update(q)
            if q["hard_ok"]:
                hard_ok += 1
        records.append(row)

    p50 = statistics.median(latencies) if latencies else None
    p95 = statistics.quantiles(latencies, n=100)[94] if len(latencies) >= 20 else None
    return {
        "endpoint": endpoint,
        "time_limit_ms": time_limit_ms,
        "restarts": restarts,
        "runs": len(seeds),
        "ok_runs": ok,
        "ok_rate": (ok / len(seeds)) if seeds else 0.0,
        "hard_ok_runs": hard_ok,
        "hard_ok_rate": (hard_ok / len(seeds)) if seeds else 0.0,
        "latency_avg_ms": (sum(latencies) / len(latencies)) if latencies else None,
        "latency_p50_ms": p50,
        "latency_p95_ms": p95,
        "records": records,
    }


def compare_pairwise(standard: Dict[str, Any], beam: Dict[str, Any]) -> Dict[str, Any]:
    by_seed_std = {r["seed"]: r for r in standard["records"]}
    by_seed_beam = {r["seed"]: r for r in beam["records"]}
    all_seeds = sorted(set(by_seed_std.keys()) & set(by_seed_beam.keys()))

    both_ok = 0
    beam_better = 0
    std_better = 0
    equal = 0
    beam_only_ok = 0
    std_only_ok = 0

    for seed in all_seeds:
        s = by_seed_std[seed]
        b = by_seed_beam[seed]
        s_ok = s.get("status") == 200 and s.get("hard_ok", False)
        b_ok = b.get("status") == 200 and b.get("hard_ok", False)
        if s_ok and b_ok:
            both_ok += 1
            sk = tuple(s["sort_key"])
            bk = tuple(b["sort_key"])
            if bk < sk:
                beam_better += 1
            elif bk > sk:
                std_better += 1
            else:
                equal += 1
        elif b_ok and not s_ok:
            beam_only_ok += 1
        elif s_ok and not b_ok:
            std_only_ok += 1

    return {
        "both_hard_ok": both_ok,
        "beam_better": beam_better,
        "standard_better": std_better,
        "equal": equal,
        "beam_only_hard_ok": beam_only_ok,
        "standard_only_hard_ok": std_only_ok,
    }


def evaluate_stability(series: Dict[str, Any], min_ok_rate: float, min_hard_ok_rate: float) -> bool:
    return (
        series.get("ok_rate", 0.0) >= min_ok_rate
        and series.get("hard_ok_rate", 0.0) >= min_hard_ok_rate
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Sweep Beam time_limit vs Standard by quality formula.")
    parser.add_argument("--base-url", default="http://127.0.0.1:8088")
    parser.add_argument("--fixture", default="tests/fixtures/multisheet_oversized.json")
    parser.add_argument("--seeds", type=int, default=40)
    parser.add_argument("--seed-start", type=int, default=1)
    parser.add_argument("--restarts", type=int, default=2)
    parser.add_argument("--standard-time-limit", type=int, default=2000)
    parser.add_argument(
        "--beam-time-limits",
        default="1000,1500,2000,2500,3000,4000,5000,7000,10000",
    )
    parser.add_argument("--grid-mm", type=float, default=5.0)
    parser.add_argument("--corridor-ok-mult", type=float, default=3.0)
    parser.add_argument("--stable-ok-rate", type=float, default=0.95)
    parser.add_argument("--stable-hard-ok-rate", type=float, default=0.95)
    parser.add_argument("--out", default="ai_docs/tmp/beam_time_sweep_quality.json")
    args = parser.parse_args()

    template = load_json(args.fixture)
    seeds = list(range(args.seed_start, args.seed_start + args.seeds))
    beam_limits = [int(x.strip()) for x in args.beam_time_limits.split(",") if x.strip()]

    print(f"[info] fixture={args.fixture}")
    print(f"[info] seeds={seeds[0]}..{seeds[-1]} (count={len(seeds)})")
    print(f"[info] standard_time_limit={args.standard_time_limit}, restarts={args.restarts}")
    print(f"[info] beam_time_limits={beam_limits}")

    standard = run_series(
        base_url=args.base_url,
        endpoint="/v1/optimize",
        template=template,
        seeds=seeds,
        time_limit_ms=args.standard_time_limit,
        restarts=args.restarts,
        grid_mm=args.grid_mm,
        corridor_ok_mult=args.corridor_ok_mult,
    )
    print(
        "[standard] "
        f"ok_rate={standard['ok_rate']:.3f} hard_ok_rate={standard['hard_ok_rate']:.3f} "
        f"lat_avg={standard['latency_avg_ms']:.1f}"
    )

    sweep = []
    first_stable_tl: Optional[int] = None
    first_better_than_std_tl: Optional[int] = None

    for tl in beam_limits:
        beam = run_series(
            base_url=args.base_url,
            endpoint="/v1/optimize/beam",
            template=template,
            seeds=seeds,
            time_limit_ms=tl,
            restarts=args.restarts,
            grid_mm=args.grid_mm,
            corridor_ok_mult=args.corridor_ok_mult,
        )
        cmp = compare_pairwise(standard, beam)
        stable = evaluate_stability(
            series=beam,
            min_ok_rate=args.stable_ok_rate,
            min_hard_ok_rate=args.stable_hard_ok_rate,
        )
        better_than_std = (
            cmp["beam_better"] > cmp["standard_better"]
            and beam["hard_ok_rate"] >= standard["hard_ok_rate"]
            and beam["ok_rate"] >= standard["ok_rate"]
        )
        if stable and first_stable_tl is None:
            first_stable_tl = tl
        if better_than_std and first_better_than_std_tl is None:
            first_better_than_std_tl = tl

        row = {
            "beam": beam,
            "pairwise_vs_standard": cmp,
            "stable": stable,
            "better_than_standard": better_than_std,
        }
        sweep.append(row)
        print(
            "[beam] "
            f"tl={tl} ok_rate={beam['ok_rate']:.3f} hard_ok_rate={beam['hard_ok_rate']:.3f} "
            f"beam_better={cmp['beam_better']} std_better={cmp['standard_better']} "
            f"stable={stable} better_than_std={better_than_std}"
        )

    report = {
        "fixture": args.fixture,
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
        "standard": standard,
        "beam_sweep": sweep,
        "first_stable_time_limit_ms": first_stable_tl,
        "first_time_limit_beam_better_than_standard_ms": first_better_than_std_tl,
    }

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print(f"[done] report saved: {args.out}")


if __name__ == "__main__":
    main()
