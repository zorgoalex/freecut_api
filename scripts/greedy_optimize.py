#!/usr/bin/env python3
"""
Greedy multi-sheet optimization with multiple subset selection strategies.

Strategies:
  - random: Random shuffle, pick items until target fill (original)
  - largest_first: Sort by area descending, pick largest items first
  - balanced: Mix of largest first + random for diversity

Algorithm:
1. For each sheet (until all items placed):
   - Sample multiple subsets using selected strategy
   - Run optimizer on each subset, track best result (min waste)
   - Early stop per sheet if no improvement for N iterations
   - Select best subset for this sheet
   - Remove placed items from remaining
2. Combine all sheets into final result
"""

import json
import random
import time
import math
import requests
import argparse
from dataclasses import dataclass
from typing import List, Optional, Dict

# Configuration
FREECUT_URL = "http://127.0.0.1:8088/v1/optimize"
TOTAL_TIME_LIMIT = 25.0  # seconds total
ITERATIONS_PER_SHEET = 30  # iterations to find best subset for each sheet
EARLY_STOP_PER_SHEET = 15  # stop sheet search if no improvement for N iterations
AREA_FILL_TARGET = 0.85  # aim to fill ~85% of sheet area with items
REQUEST_TIMEOUT = 2.0  # seconds per API request
RETRY_429 = 1
RETRY_BACKOFF_S = 0.1
MAX_CONSECUTIVE_408 = 10


@dataclass
class Item:
    id: str
    width_mm: float
    height_mm: float
    qty: int
    rotation: str
    pattern_direction: str

    @property
    def area(self) -> float:
        return self.width_mm * self.height_mm

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "width_mm": self.width_mm,
            "height_mm": self.height_mm,
            "qty": self.qty,
            "rotation": self.rotation,
            "pattern_direction": self.pattern_direction,
        }


@dataclass
class SheetResult:
    """Result for one sheet"""
    stock_id: str
    placements: List[dict]
    waste_percent: float
    placed_item_ids: List[str]
    selected_count: int
    seed_used: int
    stop_reason: str = "iterations_exhausted"


@dataclass
class RuntimeOptions:
    subset_time_limit_ms: int = 150
    subset_restarts: int = 2
    request_timeout_s: float = REQUEST_TIMEOUT
    retry_429: int = RETRY_429
    retry_backoff_s: float = RETRY_BACKOFF_S
    max_consecutive_408: int = MAX_CONSECUTIVE_408


def runtime_defaults_from_params(params: dict) -> RuntimeOptions:
    req_time_limit = int(params.get("time_limit_ms", 150))
    req_restarts = int(params.get("restarts", 2))
    base_subset_time_limit = req_time_limit // 8 if req_time_limit > 0 else 150
    subset_time_limit_ms = max(150, min(300, base_subset_time_limit))
    subset_restarts = max(1, min(2, req_restarts))
    return RuntimeOptions(
        subset_time_limit_ms=subset_time_limit_ms,
        subset_restarts=subset_restarts,
        request_timeout_s=REQUEST_TIMEOUT,
        retry_429=RETRY_429,
        retry_backoff_s=RETRY_BACKOFF_S,
        max_consecutive_408=MAX_CONSECUTIVE_408,
    )


def base_item_id(item_id: str) -> str:
    """Extract base id from expanded instance id `base#n`."""
    if "#" in item_id:
        return item_id.split("#", 1)[0]
    return item_id


def item_instance_sort_key(item_id: str) -> tuple:
    """
    Keep deterministic ordering for instance IDs.
    Expected format: `base#<int>`.
    """
    if "#" not in item_id:
        return (item_id, 0)
    base, suffix = item_id.split("#", 1)
    if suffix.isdigit():
        return (base, int(suffix))
    return (base, 0)


def extract_placed_item_ids(subset: List[Item], placements: List[dict]) -> List[str]:
    """
    Map API placements back to concrete subset instances.
    We map by base item_id + placed quantity for that base.
    """
    subset_by_base = {}
    for item in subset:
        base = base_item_id(item.id)
        subset_by_base.setdefault(base, []).append(item.id)

    for base in subset_by_base:
        subset_by_base[base].sort(key=item_instance_sort_key)

    placed_counts = {}
    for placement in placements:
        base = placement.get("item_id")
        if not base:
            continue
        placed_counts[base] = placed_counts.get(base, 0) + 1

    placed_item_ids: List[str] = []
    for base, count in placed_counts.items():
        candidates = subset_by_base.get(base, [])
        if not candidates:
            continue
        take = min(count, len(candidates))
        placed_item_ids.extend(candidates[:take])

    return placed_item_ids


def item_fits_sheet(item: Item, stock: dict, params: dict) -> bool:
    """Fast per-item feasibility check before calling API."""
    trim = params.get("trim_mm", {"left": 0, "right": 0, "top": 0, "bottom": 0})
    gap = float(params.get("kerf_mm", 0.0)) + float(params.get("spacing_mm", 0.0))
    usable_w = stock["width_mm"] - trim.get("left", 0.0) - trim.get("right", 0.0)
    usable_h = stock["height_mm"] - trim.get("top", 0.0) - trim.get("bottom", 0.0)

    w = item.width_mm + gap
    h = item.height_mm + gap
    if w <= usable_w and h <= usable_h:
        return True
    if item.rotation == "allow_90" and h <= usable_w and w <= usable_h:
        return True
    return False


def normalize_stocks(stock_input) -> List[dict]:
    if isinstance(stock_input, list):
        stocks = [dict(s) for s in stock_input]
    else:
        stocks = [dict(stock_input)]

    for idx, stock in enumerate(stocks, start=1):
        if not stock.get("id"):
            stock["id"] = f"stock_{idx}"
        stock["qty"] = int(stock.get("qty", 0))
    return stocks


def item_fits_any_stock(item: Item, stocks: List[dict], params: dict) -> bool:
    return any(item_fits_sheet(item, stock, params) for stock in stocks)


def candidate_sheet_score(result: SheetResult, stock: dict) -> tuple:
    placed_count = len(result.placements)
    placed_area = sum(float(p["width_mm"]) * float(p["height_mm"]) for p in result.placements)
    limited_priority = 1 if int(stock.get("qty", 0)) > 0 else 0
    return (placed_count, placed_area, -result.waste_percent, limited_priority)


def expand_items(items_config: List[dict]) -> List[Item]:
    """Expand qty into individual item instances"""
    expanded = []
    for item in items_config:
        for instance in range(1, item["qty"] + 1):
            expanded.append(Item(
                id=f"{item['id']}#{instance}",
                width_mm=item["width_mm"],
                height_mm=item["height_mm"],
                qty=1,
                rotation=item.get("rotation", "allow_90"),
                pattern_direction=item.get("pattern_direction", "none"),
            ))
    return expanded


# =============================================================================
# STRATEGY 1: Random selection (original)
# =============================================================================
def select_random_subset(
    items: List[Item],
    max_area: float,
    target_fill: float = AREA_FILL_TARGET
) -> List[Item]:
    """
    Random strategy: shuffle items, pick until target fill.
    Simple but requires many iterations to find good combinations.
    """
    if not items:
        return []

    target_area = max_area * target_fill
    shuffled = items.copy()
    random.shuffle(shuffled)

    selected = []
    current_area = 0.0

    for item in shuffled:
        if current_area + item.area <= max_area:
            selected.append(item)
            current_area += item.area
            if current_area >= target_area:
                break

    return selected


# =============================================================================
# STRATEGY 2: Largest first (deterministic greedy)
# =============================================================================
def select_largest_first_subset(
    items: List[Item],
    max_area: float,
    target_fill: float = AREA_FILL_TARGET,
    variation: float = 0.0
) -> List[Item]:
    """
    Largest first strategy: sort by area descending, pick largest items first.
    Better for placing difficult (large) items early.

    variation: 0.0 = pure largest first, 1.0 = random order for same-size items
    """
    if not items:
        return []

    target_area = max_area * target_fill

    # Sort by area descending, with optional random tiebreaker
    if variation > 0:
        sorted_items = sorted(
            items,
            key=lambda x: (-x.area, random.random() * variation)
        )
    else:
        sorted_items = sorted(items, key=lambda x: -x.area)

    selected = []
    current_area = 0.0

    for item in sorted_items:
        if current_area + item.area <= max_area:
            selected.append(item)
            current_area += item.area
            if current_area >= target_area:
                break

    return selected


# =============================================================================
# STRATEGY 3: Balanced (mix of largest first + random)
# =============================================================================
def select_balanced_subset(
    items: List[Item],
    max_area: float,
    target_fill: float = AREA_FILL_TARGET,
    large_ratio: float = 0.5
) -> List[Item]:
    """
    Balanced strategy:
    1. First, greedily pick largest items (up to large_ratio of target)
    2. Then, randomly fill the rest

    Combines deterministic placement of hard items with random exploration.
    """
    if not items:
        return []

    target_area = max_area * target_fill
    large_target = target_area * large_ratio

    # Sort by area descending
    sorted_items = sorted(items, key=lambda x: -x.area)

    selected = []
    current_area = 0.0
    used_indices = set()

    # Phase 1: Pick largest items up to large_ratio
    for i, item in enumerate(sorted_items):
        if current_area >= large_target:
            break
        if current_area + item.area <= max_area:
            selected.append(item)
            current_area += item.area
            used_indices.add(i)

    # Phase 2: Randomly fill the rest
    remaining = [item for i, item in enumerate(sorted_items) if i not in used_indices]
    random.shuffle(remaining)

    for item in remaining:
        if current_area + item.area <= max_area:
            selected.append(item)
            current_area += item.area
            if current_area >= target_area:
                break

    return selected


# =============================================================================
# Strategy selector
# =============================================================================
STRATEGIES = {
    "random": lambda items, max_area, iteration: select_random_subset(items, max_area),
    "largest_first": lambda items, max_area, iteration: select_largest_first_subset(
        items, max_area, variation=0.3 if iteration > 0 else 0.0
    ),
    "balanced": lambda items, max_area, iteration: select_balanced_subset(
        items, max_area, large_ratio=0.3 + random.random() * 0.4
    ),
}


def optimize_subset(
    items: List[Item],
    stock: dict,
    params: dict,
    seed: int,
    runtime: RuntimeOptions,
) -> tuple[Optional[dict], str]:
    """Run optimization for a subset of items on single sheet"""
    if not items:
        return None, "empty_subset"

    # Merge items with same base id for API request
    merged = {}
    for item in items:
        base_id = item.id.split("#")[0]
        key = (base_id, item.width_mm, item.height_mm)
        if key not in merged:
            merged[key] = {
                "id": base_id,
                "width_mm": item.width_mm,
                "height_mm": item.height_mm,
                "qty": 0,
                "rotation": item.rotation,
                "pattern_direction": item.pattern_direction,
            }
        merged[key]["qty"] += 1

    request_data = {
        "units": "mm",
        "params": {
            **params,
            "seed": seed,
            "time_limit_ms": runtime.subset_time_limit_ms,
            "restarts": runtime.subset_restarts,
        },
        "stock": [{"id": stock["id"], "width_mm": stock["width_mm"],
                   "height_mm": stock["height_mm"], "qty": 1}],
        "items": list(merged.values()),
    }

    attempts = max(0, runtime.retry_429) + 1
    for attempt in range(attempts):
        try:
            resp = requests.post(FREECUT_URL, json=request_data, timeout=runtime.request_timeout_s)
        except requests.Timeout:
            return None, "transport_timeout"
        except requests.RequestException:
            return None, "transport_error"

        if resp.status_code == 200:
            return resp.json(), "200"

        if resp.status_code == 429 and attempt < attempts - 1:
            time.sleep(max(0.0, runtime.retry_backoff_s))
            continue

        return None, str(resp.status_code)

    return None, "unknown"


def calculate_sheet_area(stock: dict, trim: dict) -> float:
    """Calculate usable sheet area after trim"""
    usable_w = stock["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
    usable_h = stock["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
    return usable_w * usable_h


def estimate_sheets_needed(total_item_area: float, sheet_area: float) -> int:
    """Estimate minimum sheets needed"""
    effective_fill = 0.75  # Account for typical waste (~25%)
    return max(1, math.ceil(total_item_area / (sheet_area * effective_fill)))


def optimize_single_sheet(
    remaining_items: List[Item],
    stock: dict,
    params: dict,
    runtime: RuntimeOptions,
    sheet_area: float,
    strategy: str = "random",
    max_iterations: int = ITERATIONS_PER_SHEET,
    early_stop: int = EARLY_STOP_PER_SHEET,
    api_status_counts: Optional[Dict[str, int]] = None,
    verbose: bool = False,
) -> Optional[SheetResult]:
    """
    Find best placement for a single sheet from remaining items.
    Tries multiple subsets using the selected strategy.
    """
    if not remaining_items:
        return None

    select_fn = STRATEGIES.get(strategy, STRATEGIES["random"])

    best_score = (-1, -1.0, float("-inf"))
    best_waste = float("inf")
    best_result = None
    best_placed_item_ids: List[str] = []
    best_selected_count = 0
    best_seed = 0
    no_improve = 0
    consecutive_408 = 0
    stop_reason = "iterations_exhausted"
    subset_size_cap: Optional[int] = None

    for iteration in range(max_iterations):
        # Select subset using chosen strategy
        subset = select_fn(remaining_items, sheet_area, iteration)
        if not subset:
            stop_reason = "empty_subset"
            break
        if subset_size_cap is not None and len(subset) > subset_size_cap:
            subset = subset[:subset_size_cap]

        # Try to optimize this subset
        seed = random.randint(1, 10_000_000)
        result, status_code = optimize_subset(subset, stock, params, seed, runtime)

        if api_status_counts is not None:
            api_status_counts[status_code] = api_status_counts.get(status_code, 0) + 1

        if status_code == "408":
            consecutive_408 += 1
            current_cap = subset_size_cap or len(subset)
            subset_size_cap = max(1, int(current_cap * 0.8))
        elif status_code == "200":
            consecutive_408 = 0
            if subset_size_cap is not None and subset_size_cap < len(remaining_items):
                subset_size_cap += 1

        if result and result.get("status") == "ok":
            solutions = result.get("solutions") or []
            if not solutions:
                no_improve += 1
                continue

            placements = solutions[0].get("placements") or []
            if not placements:
                no_improve += 1
                continue

            waste = float(result["summary"]["waste_percent"])
            placed_item_ids = extract_placed_item_ids(subset, placements)
            if not placed_item_ids:
                no_improve += 1
                continue
            placed_count = len(placed_item_ids)
            placed_area = sum(float(p["width_mm"]) * float(p["height_mm"]) for p in placements)
            score = (placed_count, placed_area, -waste)

            if score > best_score:
                best_score = score
                best_waste = waste
                best_result = result
                best_placed_item_ids = placed_item_ids
                best_selected_count = len(subset)
                best_seed = seed
                no_improve = 0

                if verbose:
                    print(
                        f"    Iter {iteration+1}: selected={len(subset)}, "
                        f"placed={placed_count}, area={placed_area:.0f}, waste={waste:.2f}% (NEW BEST)"
                    )
            else:
                no_improve += 1
        else:
            no_improve += 1

        # Early stop for this sheet
        if no_improve >= early_stop:
            stop_reason = "no_improve"
            break

        if consecutive_408 >= max(1, runtime.max_consecutive_408):
            stop_reason = "too_many_408"
            break

    if best_result is None:
        return None

    return SheetResult(
        stock_id=stock["id"],
        placements=best_result["solutions"][0]["placements"],
        waste_percent=best_waste,
        placed_item_ids=best_placed_item_ids,
        selected_count=best_selected_count,
        seed_used=best_seed,
        stop_reason=stop_reason,
    )


def greedy_optimize(
    items_config: List[dict],
    stock_input,
    params: dict,
    strategy: str = "random",
    total_time_limit: float = TOTAL_TIME_LIMIT,
    runtime: Optional[RuntimeOptions] = None,
    verbose: bool = True,
) -> dict:
    """
    Greedy multi-sheet optimization with selectable strategy.

    Strategies:
      - random: Random subset selection (original, needs many iterations)
      - largest_first: Place largest items first (deterministic, fast)
      - balanced: Mix of largest first + random (best of both)
    """
    start_time = time.time()
    runtime = runtime or runtime_defaults_from_params(params)
    stocks = normalize_stocks(stock_input)
    if not stocks:
        return {"status": "error", "message": "no stock provided"}
    stock_by_id = {stock["id"]: stock for stock in stocks}
    stock_usage = {stock["id"]: 0 for stock in stocks}

    # Expand items to individual instances
    all_items = expand_items(items_config)
    total_items = len(all_items)
    api_status_counts: Dict[str, int] = {}
    sheet_stop_reasons: Dict[str, int] = {}

    prefit_unplaced = [item for item in all_items if not item_fits_any_stock(item, stocks, params)]
    remaining = [item for item in all_items if item_fits_any_stock(item, stocks, params)]

    if verbose:
        print(f"Total items: {total_items}")
        print(f"Strategy: {strategy}")
        if prefit_unplaced:
            print(f"Pre-fit filtered (oversized for sheet): {len(prefit_unplaced)}")

    # Calculate areas
    trim = params.get("trim_mm", {"left": 0, "right": 0, "top": 0, "bottom": 0})
    max_sheet_area = max(calculate_sheet_area(stock, trim) for stock in stocks)
    total_item_area = sum(item.area for item in all_items)

    if verbose:
        print(f"Max sheet area: {max_sheet_area/1e6:.2f} m², Total items area: {total_item_area/1e6:.2f} m²")

    # Estimate sheets needed
    estimated_sheets = estimate_sheets_needed(total_item_area, max_sheet_area)
    if verbose:
        print(f"Estimated sheets: {estimated_sheets}")

    # Process sheets one by one
    sheets: List[SheetResult] = []
    sheet_num = 0

    if verbose:
        print(f"\nOptimizing sheets...")

    while remaining:
        elapsed = time.time() - start_time
        if elapsed >= total_time_limit:
            if verbose:
                print(f"Time limit reached after {sheet_num} sheets")
            break

        sheet_num += 1

        # Calculate iterations budget for this sheet
        remaining_time = total_time_limit - elapsed
        estimated_remaining_sheets = max(1, estimate_sheets_needed(
            sum(item.area for item in remaining), max_sheet_area
        ))
        time_per_sheet = remaining_time / estimated_remaining_sheets
        iterations = max(10, min(ITERATIONS_PER_SHEET, int(time_per_sheet / 0.2)))

        if verbose:
            print(f"\n  Sheet {sheet_num}: {len(remaining)} items remaining, {iterations} iterations")

        available_stocks = []
        for stock in stocks:
            qty = int(stock.get("qty", 0))
            used = stock_usage.get(stock["id"], 0)
            if qty == 0 or used < qty:
                available_stocks.append(stock)

        if not available_stocks:
            sheet_stop_reasons["no_available_stock_qty"] = sheet_stop_reasons.get("no_available_stock_qty", 0) + 1
            if verbose:
                print("    No available stock left due to qty limits")
            break

        best_sheet: Optional[SheetResult] = None
        best_score: Optional[tuple] = None
        for stock in available_stocks:
            sheet_area = calculate_sheet_area(stock, trim)
            if sheet_area <= 0:
                continue
            sheet_result = optimize_single_sheet(
                remaining,
                stock,
                params,
                runtime,
                sheet_area,
                strategy=strategy,
                max_iterations=iterations,
                api_status_counts=api_status_counts,
                verbose=verbose,
            )
            if sheet_result is None:
                continue

            score = candidate_sheet_score(sheet_result, stock)
            if best_score is None or score > best_score:
                best_score = score
                best_sheet = sheet_result

        if best_sheet is None:
            sheet_stop_reasons["no_solution_for_sheet"] = sheet_stop_reasons.get("no_solution_for_sheet", 0) + 1
            if verbose:
                print(f"    Could not place any items on sheet {sheet_num}")
            break

        sheet_result = best_sheet
        sheets.append(sheet_result)
        sheet_stop_reasons[sheet_result.stop_reason] = sheet_stop_reasons.get(sheet_result.stop_reason, 0) + 1
        stock_usage[sheet_result.stock_id] = stock_usage.get(sheet_result.stock_id, 0) + 1

        # Remove placed items from remaining
        placed_ids = set(sheet_result.placed_item_ids)
        remaining = [item for item in remaining if item.id not in placed_ids]

        if verbose:
            print(
                f"    Best[{sheet_result.stock_id}]: selected={sheet_result.selected_count}, "
                f"placed={len(sheet_result.placements)}, waste={sheet_result.waste_percent:.2f}%"
            )

    total_time = time.time() - start_time
    final_unplaced = remaining + prefit_unplaced

    # Calculate overall statistics
    total_placed = sum(len(s.placements) for s in sheets)
    total_stock_area = sum(
        calculate_sheet_area(stock_by_id[s.stock_id], trim) for s in sheets if s.stock_id in stock_by_id
    )
    total_items_area = sum(
        sum(p["width_mm"] * p["height_mm"] for p in s.placements)
        for s in sheets
    )
    overall_waste = 100 * (total_stock_area - total_items_area) / total_stock_area if total_stock_area > 0 else 0
    invariant_missing = total_items - (total_placed + len(final_unplaced))
    invariant_ok = invariant_missing == 0

    if verbose:
        print(f"\n=== RESULT ({strategy}) ===")
        print(f"Sheets used: {len(sheets)}")
        print(f"Items placed: {total_placed} / {total_items}")
        print(f"Overall waste: {overall_waste:.2f}%")
        print(f"Time: {total_time:.1f}s")
        print(f"Stock usage: {stock_usage}")
        if not invariant_ok:
            print(
                f"WARNING: item accounting mismatch, missing={invariant_missing} "
                f"(placed + unplaced != total)"
            )
        print(f"API status counts: {api_status_counts}")
        print(f"Sheet stop reasons: {sheet_stop_reasons}")

        for i, sheet in enumerate(sheets):
            print(f"  Sheet {i+1}: {len(sheet.placements)} items, waste={sheet.waste_percent:.2f}%")

        if final_unplaced:
            print(f"\nUnplaced items: {len(final_unplaced)}")
            for item in final_unplaced[:5]:
                print(f"  - {item.id} ({item.width_mm}x{item.height_mm})")
            if len(final_unplaced) > 5:
                print(f"  ... and {len(final_unplaced) - 5} more")

    return {
        "status": "ok",
        "algorithm": f"greedy_{strategy}",
        "summary": {
            "sheets_used": len(sheets),
            "items_placed": total_placed,
            "items_total": total_items,
            "overall_waste_percent": overall_waste,
            "time_seconds": total_time,
            "strategy": strategy,
            "invariant_ok": invariant_ok,
            "missing_items": invariant_missing,
            "prefit_filtered_items": len(prefit_unplaced),
            "api_status_counts": api_status_counts,
            "sheet_stop_reasons": sheet_stop_reasons,
            "stock_usage": stock_usage,
        },
        "sheets": [
            {
                "index": i,
                "stock_id": s.stock_id,
                "waste_percent": s.waste_percent,
                "placements": s.placements,
            }
            for i, s in enumerate(sheets)
        ],
        "unplaced_items": [
            {"id": item.id, "width_mm": item.width_mm, "height_mm": item.height_mm}
            for item in final_unplaced
        ],
    }


def main():
    parser = argparse.ArgumentParser(
        description="Greedy multi-sheet optimizer with multiple strategies",
        epilog="""
Strategies:
  random       - Random subset selection (original, needs many iterations)
  largest_first - Place largest items first (deterministic, fast)
  balanced     - Mix of largest first + random (best of both)

Examples:
  python greedy_optimize.py -i request.json
  python greedy_optimize.py -i request.json -s largest_first
  python greedy_optimize.py -i request.json -s balanced -t 60
"""
    )
    parser.add_argument("--input", "-i", required=True, help="Input JSON file")
    parser.add_argument(
        "--strategy", "-s",
        choices=["random", "largest_first", "balanced"],
        default="balanced",
        help="Subset selection strategy (default: balanced)"
    )
    parser.add_argument(
        "--time-limit", "-t",
        type=float,
        default=None,
        help="Time limit in seconds (default: 25s or from params)"
    )
    parser.add_argument("--subset-time-limit-ms", type=int, default=None, help="Per-subset optimize timeout, ms")
    parser.add_argument("--subset-restarts", type=int, default=None, help="Per-subset restarts")
    parser.add_argument("--request-timeout-s", type=float, default=None, help="HTTP timeout for subset requests")
    parser.add_argument("--retry-429", type=int, default=None, help="Retry count for HTTP 429 responses")
    parser.add_argument("--retry-backoff-ms", type=int, default=None, help="Backoff between 429 retries in ms")
    parser.add_argument(
        "--max-consecutive-408",
        type=int,
        default=None,
        help="Break sheet search after N consecutive 408 responses",
    )
    parser.add_argument("--quiet", "-q", action="store_true", help="Less verbose output")
    args = parser.parse_args()

    with open(args.input) as f:
        data = json.load(f)

    # Time limit priority: CLI arg > params.search_time_limit_ms > default 25s
    time_limit = args.time_limit
    if time_limit is None:
        search_time_ms = data.get("params", {}).get("search_time_limit_ms")
        if search_time_ms:
            time_limit = search_time_ms / 1000.0
        else:
            time_limit = 25.0

    if time_limit > 300:
        print(f"Warning: time limit {time_limit}s is very long, consider using 60-300s")

    cfg = data.get("params", {})
    base_runtime = runtime_defaults_from_params(cfg)
    subset_time_limit_ms = args.subset_time_limit_ms
    if subset_time_limit_ms is None:
        subset_time_limit_ms = int(cfg.get("greedy_subset_time_limit_ms", base_runtime.subset_time_limit_ms))

    subset_restarts = args.subset_restarts
    if subset_restarts is None:
        subset_restarts = int(cfg.get("greedy_subset_restarts", base_runtime.subset_restarts))

    request_timeout_s = args.request_timeout_s
    if request_timeout_s is None:
        request_timeout_s = float(cfg.get("greedy_request_timeout_s", REQUEST_TIMEOUT))

    retry_429 = args.retry_429
    if retry_429 is None:
        retry_429 = int(cfg.get("greedy_retry_429", RETRY_429))

    retry_backoff_ms = args.retry_backoff_ms
    if retry_backoff_ms is None:
        retry_backoff_ms = int(cfg.get("greedy_retry_backoff_ms", int(RETRY_BACKOFF_S * 1000)))

    max_consecutive_408 = args.max_consecutive_408
    if max_consecutive_408 is None:
        max_consecutive_408 = int(cfg.get("greedy_max_consecutive_408", MAX_CONSECUTIVE_408))

    runtime = RuntimeOptions(
        subset_time_limit_ms=max(50, subset_time_limit_ms),
        subset_restarts=max(1, subset_restarts),
        request_timeout_s=max(0.2, request_timeout_s),
        retry_429=max(0, retry_429),
        retry_backoff_s=max(0.0, retry_backoff_ms / 1000.0),
        max_consecutive_408=max(1, max_consecutive_408),
    )

    result = greedy_optimize(
        items_config=data["items"],
        stock_input=data["stock"],
        params=data["params"],
        strategy=args.strategy,
        total_time_limit=time_limit,
        runtime=runtime,
        verbose=not args.quiet,
    )

    print("\n" + json.dumps(result["summary"], indent=2))


if __name__ == "__main__":
    main()
