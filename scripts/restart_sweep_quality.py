#!/usr/bin/env python3
import argparse
import copy
import json
import statistics
import time
from typing import Any, Dict, List, Optional, Tuple

import requests

from optimize_search import calculate_internal_void_metrics

MIN_SLICE_MS = 80


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

    waste = body.get("summary", {}).get("waste_percent")
    if waste is None:
        waste = float("inf")

    placeable_ratio = compute_placeable_ratio(body)
    hard_ok = (placeable_ratio == 1.0) and (float(internal_void) == 0.0)
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


def endpoint_for_mode(mode: str) -> str:
    if mode == "standard":
        return "/v1/optimize"
    if mode == "portfolio":
        return "/v1/optimize"
    if mode == "beam":
        return "/v1/optimize/beam"
    if mode == "alns":
        return "/v1/optimize/alns"
    raise ValueError(f"unsupported mode: {mode}")


def make_payload(
    template: Dict[str, Any],
    mode: str,
    seed: int,
    time_limit_ms: int,
    restarts: int,
) -> Dict[str, Any]:
    payload = copy.deepcopy(template)
    params = payload.setdefault("params", {})
    params["seed"] = seed
    params["time_limit_ms"] = time_limit_ms
    params["restarts"] = restarts
    params["include_svg"] = False
    params["layout_mode"] = "guillotine"

    params.pop("portfolio", None)
    params.pop("beam", None)
    params.pop("alns", None)

    if mode == "portfolio":
        params["portfolio"] = {
            "enabled": True,
            "deadline_ms": time_limit_ms,
            "candidate_count": 2,
        }
    elif mode == "beam":
        params["beam"] = {
            "enabled": True,
            "deadline_ms": time_limit_ms,
            "beam_width": 2,
            "beam_depth": 2,
            "branch_factor": 2,
        }
    elif mode == "alns":
        params["alns"] = {
            "enabled": True,
            "deadline_ms": time_limit_ms,
            "iterations": 24,
            "segment_size": 6,
            "temperature_start": 1.0,
            "temperature_end": 0.12,
            "reaction_factor": 0.3,
        }
    return payload


def avg(vals: List[float]) -> Optional[float]:
    if not vals:
        return None
    return sum(vals) / len(vals)


def run_series(
    base_url: str,
    template: Dict[str, Any],
    mode: str,
    seeds: List[int],
    time_limit_ms: int,
    restarts: int,
    grid_mm: float,
    corridor_ok_mult: float,
) -> Dict[str, Any]:
    endpoint = endpoint_for_mode(mode)
    timeout_s = max(12.0, (time_limit_ms / 1000.0) + 8.0)

    records: List[Dict[str, Any]] = []
    status_counts: Dict[str, int] = {}
    latencies: List[float] = []
    hard_ok_count = 0
    restarts_used_vals: List[float] = []
    timeout_reason_counts: Dict[str, int] = {}

    for idx, seed in enumerate(seeds):
        if idx > 0 and idx % 10 == 0:
            print(
                f"[progress] mode={mode} tl={time_limit_ms} restarts={restarts} run={idx}/{len(seeds)}"
            )
        payload = make_payload(template, mode, seed, time_limit_ms, restarts)
        status, body, elapsed_ms = post_optimize(base_url, endpoint, payload, timeout_s=timeout_s)
        status_counts[str(status)] = status_counts.get(str(status), 0) + 1
        rec: Dict[str, Any] = {
            "seed": seed,
            "status": status,
            "elapsed_ms": elapsed_ms,
        }

        if status == 200:
            latencies.append(elapsed_ms)
            summary = body.get("summary", {})
            if isinstance(summary, dict):
                ru = summary.get("restarts_used")
                if isinstance(ru, (int, float)):
                    restarts_used_vals.append(float(ru))
                    rec["restarts_used"] = int(ru)
                reason = summary.get("timeout_reason")
                if isinstance(reason, str) and reason:
                    timeout_reason_counts[reason] = timeout_reason_counts.get(reason, 0) + 1
            spacing = float(payload.get("params", {}).get("spacing_mm", 0.0))
            q = compute_quality_metrics(
                body=body,
                grid_mm=grid_mm,
                spacing_mm=spacing,
                corridor_ok_mult=corridor_ok_mult,
            )
            rec.update(q)
            if q["hard_ok"]:
                hard_ok_count += 1
        records.append(rec)

    cap_single_run = max(1, time_limit_ms // MIN_SLICE_MS)
    used_eq_requested = 0
    used_lt_requested = 0
    used_gt_cap = 0
    ok_with_used = 0
    for rec in records:
        if rec.get("status") != 200:
            continue
        ru = rec.get("restarts_used")
        if not isinstance(ru, int):
            continue
        ok_with_used += 1
        if ru == restarts:
            used_eq_requested += 1
        if ru < restarts:
            used_lt_requested += 1
        if ru > cap_single_run:
            used_gt_cap += 1

    return {
        "mode": mode,
        "endpoint": endpoint,
        "time_limit_ms": time_limit_ms,
        "restarts_requested": restarts,
        "theoretical_single_run_cap_floor_t_over_80": cap_single_run,
        "runs": len(seeds),
        "ok_runs": sum(1 for r in records if r.get("status") == 200),
        "ok_rate": sum(1 for r in records if r.get("status") == 200) / len(seeds),
        "hard_ok_runs": hard_ok_count,
        "hard_ok_rate": hard_ok_count / len(seeds),
        "avg_latency_ms": avg(latencies),
        "p50_latency_ms": statistics.median(latencies) if latencies else None,
        "avg_restarts_used": avg(restarts_used_vals),
        "eq_requested_rate": (used_eq_requested / ok_with_used) if ok_with_used > 0 else None,
        "lt_requested_rate": (used_lt_requested / ok_with_used) if ok_with_used > 0 else None,
        "gt_cap_count": used_gt_cap,
        "timeout_reason_counts": timeout_reason_counts,
        "status_counts": status_counts,
        "records": records,
    }


def pairwise_compare(a: Dict[str, Any], b: Dict[str, Any], label_a: str, label_b: str) -> Dict[str, Any]:
    by_a = {r["seed"]: r for r in a.get("records", [])}
    by_b = {r["seed"]: r for r in b.get("records", [])}
    seeds = sorted(set(by_a.keys()) & set(by_b.keys()))

    both_hard_ok = 0
    a_better = 0
    b_better = 0
    equal = 0
    a_only_hard_ok = 0
    b_only_hard_ok = 0

    for s in seeds:
        ra = by_a[s]
        rb = by_b[s]
        a_ok = ra.get("status") == 200 and ra.get("hard_ok", False)
        b_ok = rb.get("status") == 200 and rb.get("hard_ok", False)
        if a_ok and b_ok:
            both_hard_ok += 1
            ka = tuple(ra["sort_key"])
            kb = tuple(rb["sort_key"])
            if ka < kb:
                a_better += 1
            elif ka > kb:
                b_better += 1
            else:
                equal += 1
        elif a_ok and not b_ok:
            a_only_hard_ok += 1
        elif b_ok and not a_ok:
            b_only_hard_ok += 1

    return {
        "seeds_compared": len(seeds),
        "both_hard_ok": both_hard_ok,
        f"{label_a}_better": a_better,
        f"{label_b}_better": b_better,
        "equal": equal,
        f"{label_a}_only_hard_ok": a_only_hard_ok,
        f"{label_b}_only_hard_ok": b_only_hard_ok,
    }


def parse_restart_list(raw: str) -> List[int]:
    vals: List[int] = []
    for x in raw.split(","):
        x = x.strip()
        if not x:
            continue
        vals.append(int(x))
    uniq = sorted(set(v for v in vals if v >= 1))
    if not uniq:
        raise ValueError("empty restart list")
    return uniq


def main() -> None:
    parser = argparse.ArgumentParser(description="Sweep restart impact for standard/portfolio/beam/alns.")
    parser.add_argument("--base-url", default="http://127.0.0.1:8088")
    parser.add_argument("--fixture", default="tests/fixtures/multisheet_oversized.json")
    parser.add_argument("--seeds", type=int, default=20)
    parser.add_argument("--seed-start", type=int, default=1)
    parser.add_argument("--restarts", default="1,2,4,8,10,16,20,100")
    parser.add_argument("--standard-time-limit", type=int, default=2000)
    parser.add_argument("--portfolio-time-limit", type=int, default=3000)
    parser.add_argument("--beam-time-limit", type=int, default=4900)
    parser.add_argument("--alns-time-limit", type=int, default=1250)
    parser.add_argument("--grid-mm", type=float, default=5.0)
    parser.add_argument("--corridor-ok-mult", type=float, default=3.0)
    parser.add_argument("--out", default="ai_docs/tmp/restart_sweep_quality.json")
    args = parser.parse_args()

    template = load_json(args.fixture)
    seeds = list(range(args.seed_start, args.seed_start + args.seeds))
    restarts = parse_restart_list(args.restarts)
    mode_tl = {
        "standard": args.standard_time_limit,
        "portfolio": args.portfolio_time_limit,
        "beam": args.beam_time_limit,
        "alns": args.alns_time_limit,
    }

    print(f"[info] fixture={args.fixture}")
    print(f"[info] seeds={seeds[0]}..{seeds[-1]} count={len(seeds)}")
    print(f"[info] restarts={restarts}")
    print(f"[info] time_limits={mode_tl}")

    def fmt_num(v: Optional[float]) -> str:
        if v is None:
            return "n/a"
        return f"{v:.1f}"

    results: Dict[str, List[Dict[str, Any]]] = {
        "standard": [],
        "portfolio": [],
        "beam": [],
        "alns": [],
    }

    for mode in ("standard", "portfolio", "beam", "alns"):
        tl = mode_tl[mode]
        for r in restarts:
            print(f"[run] mode={mode} time_limit={tl} restarts={r}")
            s = run_series(
                base_url=args.base_url,
                template=template,
                mode=mode,
                seeds=seeds,
                time_limit_ms=tl,
                restarts=r,
                grid_mm=args.grid_mm,
                corridor_ok_mult=args.corridor_ok_mult,
            )
            results[mode].append(s)
            print(
                "[done] "
                f"mode={mode} r={r} ok_rate={s['ok_rate']:.3f} hard_ok_rate={s['hard_ok_rate']:.3f} "
                f"lat={fmt_num(s['avg_latency_ms'])} used={s['avg_restarts_used']} cap={s['theoretical_single_run_cap_floor_t_over_80']} "
                f"lt_req_rate={s['lt_requested_rate']}"
            )

    def by_restart(mode: str) -> Dict[int, Dict[str, Any]]:
        return {int(s["restarts_requested"]): s for s in results[mode]}

    indexed = {m: by_restart(m) for m in results.keys()}
    std_r2 = indexed["standard"].get(2)
    if std_r2 is None:
        raise RuntimeError("restart=2 is required for standard baseline")

    comparisons: Dict[str, Dict[str, Any]] = {}
    for mode in ("standard", "portfolio", "beam", "alns"):
        comparisons[mode] = {}
        mode_r2 = indexed[mode].get(2)
        for r in restarts:
            current = indexed[mode][r]
            row: Dict[str, Any] = {}
            if mode_r2 is not None:
                row["vs_mode_r2"] = pairwise_compare(
                    current, mode_r2, f"{mode}_r{r}", f"{mode}_r2"
                )
            row["vs_standard_r2"] = pairwise_compare(
                current, std_r2, f"{mode}_r{r}", "standard_r2"
            )
            comparisons[mode][str(r)] = row

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
        "assumption_under_test": "effective_restarts roughly capped by floor(time_limit_ms / 80ms) in single-run budget",
        "config": {
            "restarts": restarts,
            "seeds": len(seeds),
            "seed_start": args.seed_start,
            "time_limits": mode_tl,
            "min_slice_ms": MIN_SLICE_MS,
        },
        "results": results,
        "comparisons": comparisons,
    }

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print(f"[done] report saved: {args.out}")


if __name__ == "__main__":
    main()
