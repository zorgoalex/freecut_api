#!/usr/bin/env python3
"""
Test script for multi-sheet optimization with oversized items.
Sends request to Freecut service and validates response.

Usage:
    python3 scripts/test_multisheet_oversized.py [--base-url URL]
"""

import json
import sys
import argparse
from pathlib import Path

try:
    import requests
except ImportError:
    print("ERROR: requests library required. Install with: pip install requests")
    sys.exit(1)

SCRIPT_DIR = Path(__file__).parent
ROOT_DIR = SCRIPT_DIR.parent
REQUEST_FILE = ROOT_DIR / "tests" / "fixtures" / "multisheet_oversized.json"


def main():
    parser = argparse.ArgumentParser(description="Test multi-sheet + oversized items")
    parser.add_argument("--base-url", default="http://127.0.0.1:8088", help="Service URL")
    parser.add_argument("--save-svg", type=str, help="Save SVG to file")
    args = parser.parse_args()

    print("=" * 60)
    print("Test: Multi-sheet optimization with oversized items")
    print("=" * 60)

    # Load request
    with open(REQUEST_FILE) as f:
        request_data = json.load(f)

    items = request_data["items"]
    total_items = sum(item["qty"] for item in items)
    oversized_items = [i for i in items if i["id"].startswith("oversized")]
    normal_items = [i for i in items if not i["id"].startswith("oversized")]

    print(f"\nRequest summary:")
    print(f"  Total item types: {len(items)}")
    print(f"  Total instances: {total_items}")
    print(f"  Normal items: {len(normal_items)} types, {sum(i['qty'] for i in normal_items)} instances")
    print(f"  Oversized items: {len(oversized_items)} types, {sum(i['qty'] for i in oversized_items)} instances")
    stock = request_data["stock"][0]
    print(f"  Stock: {stock['id']} ({stock['width_mm']}x{stock['height_mm']})")
    print(f"  Stock qty: unlimited (qty={stock.get('qty', 0)})")

    # Send request
    url = f"{args.base_url}/v1/optimize"
    print(f"\nSending request to {url} ...")
    try:
        response = requests.post(
            url,
            json=request_data,
            headers={"Content-Type": "application/json"},
            timeout=30
        )
    except requests.exceptions.ConnectionError:
        print("ERROR: Cannot connect to service. Is container running?")
        sys.exit(1)

    if response.status_code != 200:
        print(f"ERROR: HTTP {response.status_code}")
        print(response.text)
        sys.exit(1)

    result = response.json()

    # Display response
    print(f"\nResponse status: {result.get('status')}")

    summary = result.get("summary", {})
    print(f"\nSummary:")
    print(f"  Sheets used: {summary.get('used_stock_count')}")
    print(f"  Waste: {summary.get('waste_percent', 0):.2f}%")
    print(f"  Time: {summary.get('time_ms')} ms")
    print(f"  Layout mode: {summary.get('layout_mode')}")

    solutions = result.get("solutions", [])
    print(f"\nSolutions ({len(solutions)} sheets):")
    for sol in solutions:
        placements = sol.get("placements", [])
        print(f"  Sheet {sol.get('index')}: {len(placements)} items placed")

    unplaced = result.get("unplaced_items", [])
    print(f"\nUnplaced items: {len(unplaced)}")
    for item in unplaced:
        reason = item.get("reason", "unknown")
        print(f"  {item.get('item_id')} #{item.get('instance')}: "
              f"{item.get('width_mm')}x{item.get('height_mm')} ({reason})")

    # Validation
    print("\n" + "=" * 60)
    print("VALIDATION:")
    errors = []

    # 1. Should have multiple sheets (expecting 3-4)
    sheet_count = summary.get("used_stock_count", 0)
    if sheet_count < 2:
        errors.append(f"Expected 3-4 sheets, got {sheet_count}")
    else:
        print(f"  [OK] Multiple sheets used: {sheet_count}")

    # 2. Should have exactly 4 unplaced oversized items
    if len(unplaced) != 4:
        errors.append(f"Expected 4 unplaced items, got {len(unplaced)}")
    else:
        print(f"  [OK] Unplaced items count: {len(unplaced)}")

    # 3. All unplaced should be oversized
    unplaced_ids = [u.get("item_id") for u in unplaced]
    oversized_unplaced = [u for u in unplaced_ids if u.startswith("oversized")]
    if len(oversized_unplaced) != 4:
        errors.append(f"Expected all unplaced to be oversized, got: {unplaced_ids}")
    else:
        print(f"  [OK] All unplaced are oversized items")

    # 4. All unplaced should have reason "oversized"
    reasons = [u.get("reason") for u in unplaced]
    if not all(r == "oversized" for r in reasons):
        errors.append(f"Expected all reasons='oversized', got: {reasons}")
    else:
        print(f"  [OK] All unplaced have reason='oversized'")

    # 5. SVG should be present
    svg = result.get("artifacts", {}).get("svg", "")
    if not (svg.startswith("<svg") and svg.endswith("</svg>")):
        errors.append("SVG missing or invalid")
    else:
        print(f"  [OK] SVG present ({len(svg)} bytes)")

    # Save SVG if requested
    if args.save_svg and svg:
        svg_path = Path(args.save_svg)
        svg_path.parent.mkdir(parents=True, exist_ok=True)
        with open(svg_path, "w") as f:
            f.write(svg)
        print(f"\n  SVG saved to: {svg_path}")

    # Result
    print("=" * 60)
    if errors:
        print("FAILED:")
        for e in errors:
            print(f"  - {e}")
        sys.exit(1)
    else:
        print("ALL TESTS PASSED!")
        sys.exit(0)


if __name__ == "__main__":
    main()
