#!/usr/bin/env python3
"""
Benchmark script to find time_limit boundaries.
Tests how many items can be processed within default time limits.
"""

import argparse
import json
import requests
import sys
import time

BASE_URL = "http://127.0.0.1:8088"

def generate_request(num_item_types, qty_per_type=1, time_limit_ms=None, seed=None):
    """Generate request with specified number of item types."""
    items = []
    for i in range(num_item_types):
        # Vary sizes to make optimization more complex
        w = 100 + (i % 10) * 50
        h = 150 + (i % 7) * 40
        items.append({
            "id": f"item-{i+1}",
            "width_mm": float(w),
            "height_mm": float(h),
            "qty": qty_per_type,
            "rotation": "allow_90",
            "pattern_direction": "none"
        })

    params = {
        "kerf_mm": 3.0,
        "spacing_mm": 1.0,
        "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
        "restarts": 3,
        "objective": "min_waste",
        "layout_mode": "guillotine"
    }
    if seed is not None:
        params["seed"] = seed
    if time_limit_ms:
        params["time_limit_ms"] = time_limit_ms

    return {
        "units": "mm",
        "params": params,
        "stock": [{"id": "sheet", "width_mm": 2500.0, "height_mm": 1250.0, "qty": 0}],
        "items": items
    }

def test_request(request_data):
    """Send request and return (success, time_ms, status_code, error)."""
    start = time.time()
    try:
        resp = requests.post(
            f"{BASE_URL}/v1/optimize",
            json=request_data,
            headers={"Content-Type": "application/json"},
            timeout=60
        )
        elapsed = (time.time() - start) * 1000

        if resp.status_code == 200:
            data = resp.json()
            return True, data["summary"]["time_ms"], resp.status_code, None
        else:
            return False, elapsed, resp.status_code, resp.text[:200]
    except Exception as e:
        elapsed = (time.time() - start) * 1000
        return False, elapsed, 0, str(e)

def main():
    parser = argparse.ArgumentParser(description="Freecut time_limit benchmark")
    parser.add_argument(
        "--seed",
        type=int,
        default=None,
        help="Optional params.seed for deterministic runs. Omit to use server-generated seed.",
    )
    args = parser.parse_args()

    print("=" * 70)
    print("Freecut Time Limit Benchmark")
    print("=" * 70)
    if args.seed is None:
        print("Seed: server-generated (params.seed omitted)")
    else:
        print(f"Seed: {args.seed}")

    # Check service
    try:
        r = requests.get(f"{BASE_URL}/health/ready", timeout=5)
        if r.status_code != 200:
            print("ERROR: Service not ready")
            sys.exit(1)
    except:
        print("ERROR: Cannot connect to service")
        sys.exit(1)

    print("\n1. Testing with DEFAULT time_limit (2000ms)")
    print("-" * 70)
    print(f"{'Items':<10} {'Instances':<12} {'Status':<10} {'Time(ms)':<12} {'Result'}")
    print("-" * 70)

    # Test increasing item counts with default time_limit
    for num_types in [10, 50, 100, 200, 300, 500, 700, 1000, 1500, 2000, 3000, 4000, 5000]:
        req = generate_request(num_types, qty_per_type=1, seed=args.seed)
        total_instances = num_types

        success, time_ms, status, error = test_request(req)

        if success:
            result = f"OK"
        else:
            result = f"FAIL: {status} - {error[:50] if error else ''}"

        print(f"{num_types:<10} {total_instances:<12} {status:<10} {time_ms:<12.0f} {result}")

        if not success and status in [408, 422]:
            print(f"\n>>> Limit reached at {num_types} item types!")
            break

    print("\n2. Testing with INCREASED time_limit (10000ms)")
    print("-" * 70)

    for num_types in [1000, 2000, 3000, 4000, 5000]:
        req = generate_request(num_types, qty_per_type=1, time_limit_ms=10000, seed=args.seed)

        success, time_ms, status, error = test_request(req)

        if success:
            result = f"OK"
        else:
            result = f"FAIL: {status}"

        print(f"{num_types:<10} {num_types:<12} {status:<10} {time_ms:<12.0f} {result}")

    print("\n3. Testing with qty_per_type > 1 (more instances)")
    print("-" * 70)

    for num_types, qty in [(100, 10), (100, 20), (100, 50), (50, 100)]:
        req = generate_request(num_types, qty_per_type=qty, seed=args.seed)
        total = num_types * qty

        success, time_ms, status, error = test_request(req)

        if success:
            result = f"OK"
        else:
            result = f"FAIL: {status}"

        print(f"{num_types:<10} {total:<12} {status:<10} {time_ms:<12.0f} {result}")

    print("\n" + "=" * 70)
    print("Benchmark complete")

if __name__ == "__main__":
    main()
