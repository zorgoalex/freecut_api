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


def compute_placeable_ratio(body: Dict[str, Any]) -> float:
    solutions = body.get("solutions", [])
    unplaced = body.get("unplaced_items", [])
    placed = sum(len(sol.get("placements", [])) for sol in solutions)
    placeable_unplaced = sum(1 for it in unplaced if it.get("reason") != "oversized")
    total = placed + placeable_unplaced
    if total == 0:
        return 1.0
    return placed / total


def compute_quality_metrics(
    body: Dict[str, Any],
    grid_mm: float,
    spacing_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
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
        solutions=body.get("solutions", []),
        grid_mm=grid_mm,
        pad_mm=0.0,
        spacing_mm=spacing_mm,
        corridor_ok_mult=corridor_ok_mult,
    )
    waste = float(body.get("summary", {}).get("waste_percent", float("inf")))
    placeable_ratio = compute_placeable_ratio(body)
    hard_ok = placeable_ratio == 1.0 and float(internal_void) == 0.0
    sort_key = (
        float(internal_void),
        float(occupied_perimeter),
        float(void_compactness),
        int(corridor_components),
        waste,
    )
    return {
        "placeable_ratio": placeable_ratio,
        "internal_void": float(internal_void),
        "occupied_perimeter": float(occupied_perimeter),
        "void_compactness": float(void_compactness),
        "corridor_components": int(corridor_components),
        "waste_percent": waste,
        "hard_ok": hard_ok,
        "sort_key": sort_key,
    }


def build_payload(
    template: Dict[str, Any],
    seed: int,
    time_limit_ms: int,
    restarts: int,
    portfolio: bool,
    portfolio_deadline_ms: int,
    portfolio_candidate_count: int,
) -> Dict[str, Any]:
    payload = copy.deepcopy(template)
    params = payload.setdefault("params", {})
    params["seed"] = seed
    params["time_limit_ms"] = time_limit_ms
    params["restarts"] = restarts
    params["include_svg"] = False
    params["layout_mode"] = "guillotine"
    params.pop("beam", None)
    params.pop("portfolio", None)
    if portfolio:
        params["portfolio"] = {
            "enabled": True,
            "deadline_ms": portfolio_deadline_ms,
            "candidate_count": portfolio_candidate_count,
        }
    return payload


def post_optimize(base_url: str, payload: Dict[str, Any], timeout_s: float) -> Tuple[int, Dict[str, Any], float]:
    t0 = time.time()
    resp = requests.post(
        f"{base_url}/v1/optimize",
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


def run_series(
    base_url: str,
    template: Dict[str, Any],
    seeds: List[int],
    time_limit_ms: int,
    restarts: int,
    portfolio: bool,
    portfolio_deadline_ms: int,
    portfolio_candidate_count: int,
    grid_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
    rows: List[Dict[str, Any]] = []
    latencies: List[float] = []
    ok_runs = 0
    hard_ok_runs = 0
    timeout_s = max(10.0, (time_limit_ms / 1000.0) + 8.0)

    for seed in seeds:
        payload = build_payload(
            template=template,
            seed=seed,
            time_limit_ms=time_limit_ms,
            restarts=restarts,
            portfolio=portfolio,
            portfolio_deadline_ms=portfolio_deadline_ms,
            portfolio_candidate_count=portfolio_candidate_count,
        )
        status, body, latency_ms = post_optimize(base_url, payload, timeout_s)
        row: Dict[str, Any] = {"seed": seed, "status": status, "latency_ms": latency_ms}
        if status == 200:
            ok_runs += 1
            latencies.append(latency_ms)
            q = compute_quality_metrics(
                body=body,
                grid_mm=grid_mm,
                spacing_mm=float(payload.get("params", {}).get("spacing_mm", 0.0)),
                corridor_ok_mult=corridor_ok_mult,
            )
            row.update(q)
            if q["hard_ok"]:
                hard_ok_runs += 1
        rows.append(row)

    p50 = statistics.median(latencies) if latencies else None
    p95 = statistics.quantiles(latencies, n=100)[94] if len(latencies) >= 20 else None
    return {
        "mode": "portfolio" if portfolio else "standard",
        "time_limit_ms": time_limit_ms,
        "runs": len(seeds),
        "ok_runs": ok_runs,
        "ok_rate": (ok_runs / len(seeds)) if seeds else 0.0,
        "hard_ok_runs": hard_ok_runs,
        "hard_ok_rate": (hard_ok_runs / len(seeds)) if seeds else 0.0,
        "latency_avg_ms": (sum(latencies) / len(latencies)) if latencies else None,
        "latency_p50_ms": p50,
        "latency_p95_ms": p95,
        "records": rows,
    }


def pairwise(standard: Dict[str, Any], portfolio: Dict[str, Any]) -> Dict[str, int]:
    by_s = {r["seed"]: r for r in standard["records"]}
    by_p = {r["seed"]: r for r in portfolio["records"]}
    seeds = sorted(set(by_s.keys()) & set(by_p.keys()))

    both_hard_ok = 0
    portfolio_better = 0
    standard_better = 0
    equal = 0
    portfolio_only_hard_ok = 0
    standard_only_hard_ok = 0

    for seed in seeds:
        s = by_s[seed]
        p = by_p[seed]
        s_ok = s.get("status") == 200 and s.get("hard_ok", False)
        p_ok = p.get("status") == 200 and p.get("hard_ok", False)
        if s_ok and p_ok:
            both_hard_ok += 1
            ks = tuple(s["sort_key"])
            kp = tuple(p["sort_key"])
            if kp < ks:
                portfolio_better += 1
            elif kp > ks:
                standard_better += 1
            else:
                equal += 1
        elif p_ok and not s_ok:
            portfolio_only_hard_ok += 1
        elif s_ok and not p_ok:
            standard_only_hard_ok += 1

    return {
        "both_hard_ok": both_hard_ok,
        "portfolio_better": portfolio_better,
        "standard_better": standard_better,
        "equal": equal,
        "portfolio_only_hard_ok": portfolio_only_hard_ok,
        "standard_only_hard_ok": standard_only_hard_ok,
    }


def stable(series: Dict[str, Any], min_ok_rate: float, min_hard_ok_rate: float) -> bool:
    return (
        series.get("ok_rate", 0.0) >= min_ok_rate
        and series.get("hard_ok_rate", 0.0) >= min_hard_ok_rate
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Sweep portfolio time_limit vs standard by quality formula.")
    parser.add_argument("--base-url", default="http://127.0.0.1:8088")
    parser.add_argument("--fixture", default="tests/fixtures/multisheet_oversized.json")
    parser.add_argument("--seeds", type=int, default=30)
    parser.add_argument("--seed-start", type=int, default=1)
    parser.add_argument("--restarts", type=int, default=2)
    parser.add_argument("--standard-time-limit", type=int, default=2000)
    parser.add_argument("--portfolio-candidate-count", type=int, default=2)
    parser.add_argument(
        "--portfolio-time-limits",
        default="1000,1500,2000,2500,3000,3500,4000,4500,5000",
    )
    parser.add_argument("--grid-mm", type=float, default=5.0)
    parser.add_argument("--corridor-ok-mult", type=float, default=3.0)
    parser.add_argument("--stable-ok-rate", type=float, default=0.95)
    parser.add_argument("--stable-hard-ok-rate", type=float, default=0.95)
    parser.add_argument("--out", default="ai_docs/tmp/portfolio_time_sweep_quality.json")
    args = parser.parse_args()

    template = load_json(args.fixture)
    seeds = list(range(args.seed_start, args.seed_start + args.seeds))
    portfolio_limits = [int(x.strip()) for x in args.portfolio_time_limits.split(",") if x.strip()]

    print(f"[info] fixture={args.fixture}")
    print(f"[info] seeds={seeds[0]}..{seeds[-1]} (count={len(seeds)})")
    print(f"[info] standard_time_limit={args.standard_time_limit}, restarts={args.restarts}")
    print(f"[info] portfolio_time_limits={portfolio_limits}")

    standard = run_series(
        base_url=args.base_url,
        template=template,
        seeds=seeds,
        time_limit_ms=args.standard_time_limit,
        restarts=args.restarts,
        portfolio=False,
        portfolio_deadline_ms=args.standard_time_limit,
        portfolio_candidate_count=args.portfolio_candidate_count,
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
    first_better_tl: Optional[int] = None

    for tl in portfolio_limits:
        portfolio = run_series(
            base_url=args.base_url,
            template=template,
            seeds=seeds,
            time_limit_ms=tl,
            restarts=args.restarts,
            portfolio=True,
            portfolio_deadline_ms=tl,
            portfolio_candidate_count=args.portfolio_candidate_count,
            grid_mm=args.grid_mm,
            corridor_ok_mult=args.corridor_ok_mult,
        )
        pw = pairwise(standard, portfolio)
        is_stable = stable(portfolio, args.stable_ok_rate, args.stable_hard_ok_rate)
        better = (
            pw["portfolio_better"] > pw["standard_better"]
            and portfolio["ok_rate"] >= standard["ok_rate"]
            and portfolio["hard_ok_rate"] >= standard["hard_ok_rate"]
        )
        if is_stable and first_stable_tl is None:
            first_stable_tl = tl
        if better and first_better_tl is None:
            first_better_tl = tl

        sweep.append(
            {
                "portfolio": portfolio,
                "pairwise_vs_standard": pw,
                "stable": is_stable,
                "better_than_standard": better,
            }
        )
        print(
            "[portfolio] "
            f"tl={tl} ok_rate={portfolio['ok_rate']:.3f} hard_ok_rate={portfolio['hard_ok_rate']:.3f} "
            f"portfolio_better={pw['portfolio_better']} std_better={pw['standard_better']} "
            f"stable={is_stable} better_than_std={better}"
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
        "portfolio_sweep": sweep,
        "first_stable_time_limit_ms": first_stable_tl,
        "first_time_limit_portfolio_better_than_standard_ms": first_better_tl,
    }

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print(f"[done] report saved: {args.out}")


if __name__ == "__main__":
    main()
