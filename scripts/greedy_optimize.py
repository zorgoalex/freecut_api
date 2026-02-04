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
from typing import List, Dict, Tuple, Optional, Callable
import copy

# Configuration
FREECUT_URL = "http://127.0.0.1:8088/v1/optimize"
TOTAL_TIME_LIMIT = 25.0  # seconds total
ITERATIONS_PER_SHEET = 30  # iterations to find best subset for each sheet
EARLY_STOP_PER_SHEET = 15  # stop sheet search if no improvement for N iterations
AREA_FILL_TARGET = 0.85  # aim to fill ~85% of sheet area with items
REQUEST_TIMEOUT = 2.0  # seconds per API request


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
    placements: List[dict]
    waste_percent: float
    items_used: List[Item]
    seed_used: int


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
) -> Optional[dict]:
    """Run optimization for a subset of items on single sheet"""
    if not items:
        return None

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
            "time_limit_ms": 150,  # Fast optimization per subset
            "restarts": 2,
        },
        "stock": [{"id": stock["id"], "width_mm": stock["width_mm"],
                   "height_mm": stock["height_mm"], "qty": 1}],
        "items": list(merged.values()),
    }

    try:
        resp = requests.post(FREECUT_URL, json=request_data, timeout=REQUEST_TIMEOUT)
        if resp.status_code == 200:
            return resp.json()
    except Exception:
        pass

    return None


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
    sheet_area: float,
    strategy: str = "random",
    max_iterations: int = ITERATIONS_PER_SHEET,
    early_stop: int = EARLY_STOP_PER_SHEET,
    verbose: bool = False,
) -> Optional[SheetResult]:
    """
    Find best placement for a single sheet from remaining items.
    Tries multiple subsets using the selected strategy.
    """
    if not remaining_items:
        return None

    select_fn = STRATEGIES.get(strategy, STRATEGIES["random"])

    best_waste = float('inf')
    best_result = None
    best_items = []
    best_seed = 0
    no_improve = 0

    for iteration in range(max_iterations):
        # Select subset using chosen strategy
        subset = select_fn(remaining_items, sheet_area, iteration)
        if not subset:
            break

        # Try to optimize this subset
        seed = random.randint(1, 10_000_000)
        result = optimize_subset(subset, stock, params, seed)

        if result and result.get("status") == "ok":
            waste = result["summary"]["waste_percent"]

            if waste < best_waste:
                best_waste = waste
                best_result = result
                best_items = subset.copy()
                best_seed = seed
                no_improve = 0

                if verbose:
                    print(f"    Iter {iteration+1}: {len(subset)} items, waste={waste:.2f}% (NEW BEST)")
            else:
                no_improve += 1
        else:
            no_improve += 1

        # Early stop for this sheet
        if no_improve >= early_stop:
            break

    if best_result is None:
        return None

    return SheetResult(
        placements=best_result["solutions"][0]["placements"],
        waste_percent=best_waste,
        items_used=best_items,
        seed_used=best_seed,
    )


def greedy_optimize(
    items_config: List[dict],
    stock: dict,
    params: dict,
    strategy: str = "random",
    total_time_limit: float = TOTAL_TIME_LIMIT,
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

    # Expand items to individual instances
    all_items = expand_items(items_config)
    total_items = len(all_items)

    if verbose:
        print(f"Total items: {total_items}")
        print(f"Strategy: {strategy}")

    # Calculate areas
    trim = params.get("trim_mm", {"left": 0, "right": 0, "top": 0, "bottom": 0})
    sheet_area = calculate_sheet_area(stock, trim)
    total_item_area = sum(item.area for item in all_items)

    if verbose:
        print(f"Sheet area: {sheet_area/1e6:.2f} m², Total items area: {total_item_area/1e6:.2f} m²")

    # Estimate sheets needed
    estimated_sheets = estimate_sheets_needed(total_item_area, sheet_area)
    if verbose:
        print(f"Estimated sheets: {estimated_sheets}")

    # Process sheets one by one
    remaining = all_items.copy()
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
            sum(item.area for item in remaining), sheet_area
        ))
        time_per_sheet = remaining_time / estimated_remaining_sheets
        iterations = max(10, min(ITERATIONS_PER_SHEET, int(time_per_sheet / 0.2)))

        if verbose:
            print(f"\n  Sheet {sheet_num}: {len(remaining)} items remaining, {iterations} iterations")

        # Find best placement for this sheet
        sheet_result = optimize_single_sheet(
            remaining,
            stock,
            params,
            sheet_area,
            strategy=strategy,
            max_iterations=iterations,
            verbose=verbose,
        )

        if sheet_result is None:
            if verbose:
                print(f"    Could not place any items on sheet {sheet_num}")
            break

        sheets.append(sheet_result)

        # Remove placed items from remaining
        placed_ids = {item.id for item in sheet_result.items_used}
        remaining = [item for item in remaining if item.id not in placed_ids]

        if verbose:
            print(f"    Best: {len(sheet_result.placements)} items, waste={sheet_result.waste_percent:.2f}%")

    total_time = time.time() - start_time

    # Calculate overall statistics
    total_placed = sum(len(s.placements) for s in sheets)
    total_stock_area = len(sheets) * sheet_area
    total_items_area = sum(
        sum(p["width_mm"] * p["height_mm"] for p in s.placements)
        for s in sheets
    )
    overall_waste = 100 * (total_stock_area - total_items_area) / total_stock_area if total_stock_area > 0 else 0

    if verbose:
        print(f"\n=== RESULT ({strategy}) ===")
        print(f"Sheets used: {len(sheets)}")
        print(f"Items placed: {total_placed} / {total_items}")
        print(f"Overall waste: {overall_waste:.2f}%")
        print(f"Time: {total_time:.1f}s")

        for i, sheet in enumerate(sheets):
            print(f"  Sheet {i+1}: {len(sheet.placements)} items, waste={sheet.waste_percent:.2f}%")

        if remaining:
            print(f"\nUnplaced items: {len(remaining)}")
            for item in remaining[:5]:
                print(f"  - {item.id} ({item.width_mm}x{item.height_mm})")
            if len(remaining) > 5:
                print(f"  ... and {len(remaining) - 5} more")

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
        },
        "sheets": [
            {
                "index": i,
                "waste_percent": s.waste_percent,
                "placements": s.placements,
            }
            for i, s in enumerate(sheets)
        ],
        "unplaced_items": [
            {"id": item.id, "width_mm": item.width_mm, "height_mm": item.height_mm}
            for item in remaining
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

    result = greedy_optimize(
        items_config=data["items"],
        stock=data["stock"][0],
        params=data["params"],
        strategy=args.strategy,
        total_time_limit=time_limit,
        verbose=not args.quiet,
    )

    print("\n" + json.dumps(result["summary"], indent=2))


if __name__ == "__main__":
    main()
