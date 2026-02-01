#!/usr/bin/env python3
"""
Greedy multi-sheet optimization with random subset sampling.

Algorithm:
1. Estimate how many sheets needed based on total item area
2. For each sheet:
   - Sample random subsets of remaining items that fit by area
   - Run optimizer on each subset, track best result (min waste)
   - Use time budget per sheet, early stopping if no improvement
3. Combine all sheets into final result

Time budget: ~25 seconds total for entire optimization.
"""

import json
import random
import time
import math
import requests
import argparse
from dataclasses import dataclass
from typing import List, Dict, Tuple, Optional
import copy

# Configuration
FREECUT_URL = "http://127.0.0.1:8088/v1/optimize"
TOTAL_TIME_LIMIT = 25.0  # seconds
EARLY_STOP_NO_IMPROVE = 20  # stop if no improvement for N iterations
AREA_FILL_TARGET = 0.85  # aim to fill ~85% of sheet area with items
MIN_ITERATIONS_PER_SHEET = 10
REQUEST_TIMEOUT = 2.0  # seconds per request


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
    items_used: List[Tuple[str, int]]  # (item_id, instance)
    svg_fragment: str


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


def select_random_subset(
    items: List[Item],
    max_area: float,
    target_fill: float = AREA_FILL_TARGET
) -> List[Item]:
    """
    Select random subset of items that fits within max_area.
    Tries to fill close to target_fill (e.g. 85%) of max_area.
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


def optimize_subset(
    items: List[Item],
    stock: dict,
    params: dict,
    seed: int,
) -> Optional[dict]:
    """Run optimization for a subset of items on single sheet"""
    if not items:
        return None

    # Merge items with same base id
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
            "time_limit_ms": 100,  # Fast optimization
            "restarts": 1,
        },
        "stock": [{"id": stock["id"], "width_mm": stock["width_mm"],
                   "height_mm": stock["height_mm"], "qty": 1}],
        "items": list(merged.values()),
    }

    try:
        resp = requests.post(FREECUT_URL, json=request_data, timeout=REQUEST_TIMEOUT)
        if resp.status_code == 200:
            return resp.json()
    except Exception as e:
        pass

    return None


def calculate_sheet_area(stock: dict, trim: dict) -> float:
    """Calculate usable sheet area after trim"""
    usable_w = stock["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
    usable_h = stock["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
    return usable_w * usable_h


def estimate_sheets_needed(total_item_area: float, sheet_area: float) -> int:
    """Estimate minimum sheets needed"""
    # Account for typical waste (~20-30%)
    effective_fill = 0.75
    return max(1, math.ceil(total_item_area / (sheet_area * effective_fill)))


def optimize_full_sequence(
    items: List[Item],
    stock: dict,
    params: dict,
    sheet_area: float,
    first_subset: List[Item],
    verbose: bool = False,
) -> Tuple[float, List[SheetResult]]:
    """
    Optimize a full sequence starting with first_subset.
    Returns (total_waste_percent, list_of_sheet_results).
    """
    remaining = [item for item in items if item not in first_subset]
    sheets = []

    # First sheet
    seed = random.randint(1, 10_000_000)
    result = optimize_subset(first_subset, stock, params, seed)

    if not result or result.get("status") != "ok":
        return float('inf'), []

    sol = result["solutions"][0]
    sheets.append(SheetResult(
        placements=sol["placements"],
        waste_percent=result["summary"]["waste_percent"],
        items_used=[(item.id, 1) for item in first_subset],
        svg_fragment="",
    ))

    # Remaining sheets - simple greedy for speed
    while remaining:
        subset = select_random_subset(remaining, sheet_area, target_fill=0.90)
        if not subset:
            break

        seed = random.randint(1, 10_000_000)
        result = optimize_subset(subset, stock, params, seed)

        if result and result.get("status") == "ok":
            sol = result["solutions"][0]
            sheets.append(SheetResult(
                placements=sol["placements"],
                waste_percent=result["summary"]["waste_percent"],
                items_used=[(item.id, 1) for item in subset],
                svg_fragment="",
            ))
            remaining = [item for item in remaining if item not in subset]
        else:
            break

    # Calculate total waste
    total_stock_area = len(sheets) * sheet_area
    total_items_area = sum(
        sum(p["width_mm"] * p["height_mm"] for p in s.placements)
        for s in sheets
    )
    total_waste = 100 * (total_stock_area - total_items_area) / total_stock_area if total_stock_area > 0 else 0

    return total_waste, sheets


def greedy_optimize(
    items_config: List[dict],
    stock: dict,
    params: dict,
    total_time_limit: float = TOTAL_TIME_LIMIT,
    verbose: bool = True,
) -> dict:
    """
    Greedy optimization with look-ahead.

    Tries multiple first-sheet combinations and picks the one
    that gives best OVERALL waste (not just first sheet).
    """
    start_time = time.time()

    # Expand items to individual instances
    all_items = expand_items(items_config)
    total_items = len(all_items)

    if verbose:
        print(f"Total items: {total_items}")

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

    # Search for best first-sheet combination
    best_overall_waste = float('inf')
    best_sheets: List[SheetResult] = []
    best_remaining: List[Item] = all_items.copy()

    iterations = 0
    no_improve_count = 0

    if verbose:
        print(f"\nSearching for optimal combination...")

    while True:
        elapsed = time.time() - start_time
        if elapsed >= total_time_limit:
            if verbose:
                print(f"Time limit reached after {iterations} iterations")
            break

        if no_improve_count >= EARLY_STOP_NO_IMPROVE * 2 and iterations >= MIN_ITERATIONS_PER_SHEET:
            if verbose:
                print(f"Early stop: no improvement for {no_improve_count} iterations")
            break

        # Try random first-sheet subset
        first_subset = select_random_subset(all_items, sheet_area)
        if not first_subset:
            break

        # Evaluate full sequence with this first subset
        total_waste, sheets = optimize_full_sequence(
            all_items, stock, params, sheet_area, first_subset, verbose=False
        )

        iterations += 1

        if total_waste < best_overall_waste:
            best_overall_waste = total_waste
            best_sheets = sheets
            # Calculate remaining items
            placed_items = set()
            for sheet in sheets:
                for item_id, _ in sheet.items_used:
                    placed_items.add(item_id)
            best_remaining = [item for item in all_items if item.id not in placed_items]

            no_improve_count = 0
            if verbose:
                placed = sum(len(s.placements) for s in sheets)
                print(f"  Iter {iterations}: {len(sheets)} sheets, waste={total_waste:.2f}%, placed={placed} (NEW BEST)")
        else:
            no_improve_count += 1

    total_time = time.time() - start_time

    # Build final result
    total_placed = sum(len(s.placements) for s in best_sheets)

    if verbose:
        print(f"\n=== RESULT ===")
        print(f"Sheets used: {len(best_sheets)}")
        print(f"Items placed: {total_placed} / {total_items}")
        print(f"Overall waste: {best_overall_waste:.2f}%")
        print(f"Iterations: {iterations}")
        print(f"Time: {total_time:.1f}s")

        for i, sheet in enumerate(best_sheets):
            print(f"  Sheet {i+1}: {len(sheet.placements)} items, waste={sheet.waste_percent:.2f}%")

        if best_remaining:
            print(f"\nUnplaced items: {len(best_remaining)}")
            for item in best_remaining[:5]:
                print(f"  - {item.id} ({item.width_mm}x{item.height_mm})")
            if len(best_remaining) > 5:
                print(f"  ... and {len(best_remaining) - 5} more")

    return {
        "status": "ok",
        "algorithm": "greedy_lookahead",
        "summary": {
            "sheets_used": len(best_sheets),
            "items_placed": total_placed,
            "items_total": total_items,
            "overall_waste_percent": best_overall_waste,
            "iterations": iterations,
            "time_seconds": total_time,
        },
        "sheets": [
            {
                "index": i,
                "waste_percent": s.waste_percent,
                "placements": s.placements,
            }
            for i, s in enumerate(best_sheets)
        ],
        "unplaced_items": [
            {"id": item.id, "width_mm": item.width_mm, "height_mm": item.height_mm}
            for item in best_remaining
        ],
    }


def main():
    parser = argparse.ArgumentParser(
        description="Greedy multi-sheet optimizer with random sampling",
        epilog="Example: python greedy_optimize.py -i request.json -t 60"
    )
    parser.add_argument("--input", "-i", required=True, help="Input JSON file with items and stock")
    parser.add_argument(
        "--time-limit", "-t",
        type=float,
        default=None,
        help="Time limit in seconds (overrides params.search_time_limit_ms)"
    )
    parser.add_argument("--quiet", "-q", action="store_true", help="Less verbose output")
    args = parser.parse_args()

    with open(args.input) as f:
        data = json.load(f)

    # Time limit priority: CLI arg > params.search_time_limit_ms > default 25s
    time_limit = args.time_limit
    if time_limit is None:
        # Check if specified in params
        search_time_ms = data.get("params", {}).get("search_time_limit_ms")
        if search_time_ms:
            time_limit = search_time_ms / 1000.0
        else:
            time_limit = 25.0  # Default

    if time_limit > 300:
        print(f"Warning: time limit {time_limit}s is very long, consider using 60-300s")

    result = greedy_optimize(
        items_config=data["items"],
        stock=data["stock"][0],
        params=data["params"],
        total_time_limit=time_limit,
        verbose=not args.quiet,
    )

    print("\n" + json.dumps(result["summary"], indent=2))


if __name__ == "__main__":
    main()
