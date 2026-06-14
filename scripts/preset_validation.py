#!/usr/bin/env python3
"""
Preset Validation Script
Compares DEFAULT params vs RECOMMENDED PRESET params across fixtures and layout modes.
Generates SVG outputs and quality metrics for visual comparison.
"""
import copy
import json
import math
import os
import sys
import time
from collections import deque
from pathlib import Path

import requests

# ============================================================================
# CONFIGURATION
# ============================================================================
BASE_URL = os.getenv("FREECUT_API_URL", "http://127.0.0.1:8088")
SEEDS_COUNT = 20  # Number of seeds to test per configuration
OUT_ROOT = Path("ai_docs/tmp/preset_validation")

FIXTURES = {
    "oversized": "tests/fixtures/multisheet_oversized.json",
    "varied_4sheets": "tests/fixtures/multisheet_varied_4sheets.json",
}

# Time budgets per fixture (ms) - tuned from last_state.md findings
TIME_BUDGETS = {
    "oversized": {"standard": 2000, "portfolio": 3000, "beam": 4900, "alns": 1250},
    "varied_4sheets": {"standard": 6000, "portfolio": 6000, "beam": 12000, "alns": 6000},
}

# DEFAULT profile: what the service uses out-of-the-box (no tuning)
DEFAULT_PROFILE = {
    "ga_profile": "balanced",
    "sla_profile": "balanced",
    # No fitness_weights, no placement_bias, no placement_heuristic
}

# RECOMMENDED preset from last_state.md section 36 (guillotine)
PRESET_GUILLOTINE = {
    "ga_profile": "balanced",
    "sla_profile": "balanced",
    "placement_heuristic": "best_short_side",
    "fitness_weights": {"waste": 0.50, "void": 0.20, "compactness": 0.10, "perimeter": 0.20},
    "placement_bias": {"bbox_weight": 0.05, "fragmentation_penalty": 0.10, "tie_break_jitter": 0.01},
}

# RECOMMENDED preset for nested (from section 37)
PRESET_NESTED = {
    "ga_profile": "balanced",
    "sla_profile": "balanced",
    "placement_heuristic": "bottom_left",
    "fitness_weights": {"waste": 0.50, "void": 0.20, "compactness": 0.10, "perimeter": 0.20},
    "placement_bias": {"bbox_weight": 0.05, "fragmentation_penalty": 0.08, "tie_break_jitter": 0.01},
}

ALGORITHMS = ["standard"]  # Start with standard; can add portfolio/beam/alns

# ============================================================================
# METRICS (from optimize_search.py)
# ============================================================================
def build_occupancy_grid(placements, usable_w, usable_h, grid_mm, pad_mm):
    cols = max(1, int(math.ceil(usable_w / grid_mm)))
    rows = max(1, int(math.ceil(usable_h / grid_mm)))
    grid = [bytearray(cols) for _ in range(rows)]
    for placement in placements:
        x0 = placement['x_mm'] - pad_mm
        y0 = placement['y_mm'] - pad_mm
        x1 = placement['x_mm'] + placement['width_mm'] + pad_mm
        y1 = placement['y_mm'] + placement['height_mm'] + pad_mm
        if x1 <= 0 or y1 <= 0 or x0 >= usable_w or y0 >= usable_h:
            continue
        gx0 = max(0, int(math.floor(x0 / grid_mm)))
        gy0 = max(0, int(math.floor(y0 / grid_mm)))
        gx1 = min(cols - 1, int(math.ceil(x1 / grid_mm)) - 1)
        gy1 = min(rows - 1, int(math.ceil(y1 / grid_mm)) - 1)
        if gx1 < gx0 or gy1 < gy0:
            continue
        fill = b'\x01' * (gx1 - gx0 + 1)
        for gy in range(gy0, gy1 + 1):
            row = grid[gy]
            row[gx0:gx1 + 1] = fill
    return grid, rows, cols

def flood_external_voids(grid, rows, cols):
    q = deque()
    def enqueue(r, c):
        if grid[r][c] == 0:
            grid[r][c] = 2
            q.append((r, c))
    for c in range(cols):
        enqueue(0, c)
        if rows > 1:
            enqueue(rows - 1, c)
    for r in range(rows):
        enqueue(r, 0)
        if cols > 1:
            enqueue(r, cols - 1)
    while q:
        r, c = q.popleft()
        if r > 0: enqueue(r - 1, c)
        if r + 1 < rows: enqueue(r + 1, c)
        if c > 0: enqueue(r, c - 1)
        if c + 1 < cols: enqueue(r, c + 1)

def calculate_bbox_metrics(placements):
    if not placements:
        return None
    min_x = float('inf'); min_y = float('inf')
    max_x = float('-inf'); max_y = float('-inf')
    total_parts_area = 0.0
    for p in placements:
        min_x = min(min_x, p['x_mm']); min_y = min(min_y, p['y_mm'])
        max_x = max(max_x, p['x_mm'] + p['width_mm']); max_y = max(max_y, p['y_mm'] + p['height_mm'])
        total_parts_area += p['width_mm'] * p['height_mm']
    return {'min_x': min_x, 'min_y': min_y, 'max_x': max_x, 'max_y': max_y, 'total_parts_area': total_parts_area}

def compute_quality_metrics(body, grid_mm=5.0, spacing_mm=0.5, corridor_ok_mult=3.0):
    """Compute visual quality metrics for a solution."""
    solutions = body.get('solutions', [])
    pad_mm = 0.0
    total_internal_area = 0.0
    total_exposure_penalty = 0.0
    total_corridor_area = 0.0
    total_corridor_components = 0
    total_occupied_perimeter = 0
    total_void_perimeter = 0
    total_void_cells = 0
    corridor_ok_mm = max(0.0, spacing_mm * corridor_ok_mult)

    for solution in solutions:
        placements = solution.get('placements', [])
        if not placements:
            continue
        trim = solution.get('trim_mm') or {}
        usable_w = solution['width_mm'] - trim.get('left', 0.0) - trim.get('right', 0.0)
        usable_h = solution['height_mm'] - trim.get('top', 0.0) - trim.get('bottom', 0.0)
        if usable_w <= 0 or usable_h <= 0:
            continue

        grid, rows, cols = build_occupancy_grid(placements, usable_w, usable_h, grid_mm, pad_mm)
        flood_external_voids(grid, rows, cols)
        bbox = calculate_bbox_metrics(placements)
        if not bbox:
            continue
        bx0 = max(0, int(math.floor(bbox['min_x'] / grid_mm)))
        by0 = max(0, int(math.floor(bbox['min_y'] / grid_mm)))
        bx1 = min(cols - 1, int(math.ceil(bbox['max_x'] / grid_mm)) - 1)
        by1 = min(rows - 1, int(math.ceil(bbox['max_y'] / grid_mm)) - 1)

        # Internal voids (cells=0, completely surrounded)
        internal_cells = 0
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    internal_cells += 1
                    grid[r][c] = 3
                    q = deque([(r, c)])
                    while q:
                        cr, cc = q.popleft()
                        for nr, nc in [(cr-1,cc),(cr+1,cc),(cr,cc-1),(cr,cc+1)]:
                            if 0 <= nr < rows and 0 <= nc < cols and grid[nr][nc] == 0:
                                grid[nr][nc] = 3
                                q.append((nr, nc))
        total_internal_area += internal_cells * (grid_mm * grid_mm)

        # Corridor components
        for r in range(by0, by1 + 1):
            for c in range(bx0, bx1 + 1):
                if grid[r][c] == 2:  # External void inside bbox = corridor
                    total_corridor_components += 1
                    grid[r][c] = 4
                    q = deque([(r, c)])
                    while q:
                        cr, cc = q.popleft()
                        for nr, nc in [(cr-1,cc),(cr+1,cc),(cr,cc-1),(cr,cc+1)]:
                            if by0 <= nr <= by1 and bx0 <= nc <= bx1 and grid[nr][nc] == 2:
                                grid[nr][nc] = 4
                                q.append((nr, nc))

        # Occupied perimeter
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 1:
                    if r == 0 or grid[r-1][c] != 1: total_occupied_perimeter += 1
                    if r == rows-1 or grid[r+1][c] != 1: total_occupied_perimeter += 1
                    if c == 0 or grid[r][c-1] != 1: total_occupied_perimeter += 1
                    if c == cols-1 or grid[r][c+1] != 1: total_occupied_perimeter += 1

        # Void compactness
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] != 1:
                    total_void_cells += 1
                    if r > 0 and grid[r-1][c] == 1: total_void_perimeter += 1
                    if r < rows-1 and grid[r+1][c] == 1: total_void_perimeter += 1
                    if c > 0 and grid[r][c-1] == 1: total_void_perimeter += 1
                    if c < cols-1 and grid[r][c+1] == 1: total_void_perimeter += 1

    void_compactness = total_void_perimeter / total_void_cells if total_void_cells > 0 else 0.0
    waste = body.get('summary', {}).get('waste_percent', float('inf'))
    used_sheets = body.get('summary', {}).get('used_stock_count', 0)

    # Placeable ratio
    unplaced = body.get('unplaced_items', [])
    placed = sum(len(s.get('placements', [])) for s in solutions)
    placeable_unplaced = sum(1 for it in unplaced if it.get('reason') != 'oversized')
    total_placeable = placed + placeable_unplaced
    placeable_ratio = placed / total_placeable if total_placeable > 0 else 1.0

    hard_ok = (placeable_ratio == 1.0) and (total_internal_area == 0.0)

    return {
        'placeable_ratio': placeable_ratio,
        'internal_void_mm2': total_internal_area,
        'occupied_perimeter': total_occupied_perimeter * grid_mm,
        'void_compactness': void_compactness,
        'corridor_components': total_corridor_components,
        'waste_percent': waste,
        'used_sheets': used_sheets,
        'hard_ok': hard_ok,
        'sort_key': (
            total_internal_area,
            total_occupied_perimeter * grid_mm,
            void_compactness,
            total_corridor_components,
            waste,
        ),
    }


# ============================================================================
# MAIN LOGIC
# ============================================================================
def load_fixture(path):
    with open(path, 'r') as f:
        return json.load(f)

def build_request(fixture, layout_mode, algorithm, profile, seed, time_budget):
    """Build a request payload with the given profile and parameters."""
    payload = copy.deepcopy(fixture)
    params = payload.setdefault('params', {})
    params['layout_mode'] = layout_mode
    params['seed'] = seed
    params['restarts'] = 2
    params['include_svg'] = True
    params['time_limit_ms'] = time_budget

    # Remove algorithm-specific keys first
    params.pop('portfolio', None)
    params.pop('beam', None)
    params.pop('alns', None)
    params.pop('fitness_weights', None)
    params.pop('placement_bias', None)
    params.pop('placement_heuristic', None)
    params.pop('ga_profile', None)
    params.pop('sla_profile', None)

    # Apply profile
    if 'ga_profile' in profile:
        params['ga_profile'] = profile['ga_profile']
    if 'sla_profile' in profile:
        params['sla_profile'] = profile['sla_profile']
    if 'placement_heuristic' in profile:
        params['placement_heuristic'] = profile['placement_heuristic']
    if 'fitness_weights' in profile:
        params['fitness_weights'] = profile['fitness_weights']
    if 'placement_bias' in profile:
        params['placement_bias'] = profile['placement_bias']

    # Algorithm-specific envelope
    if algorithm == 'portfolio':
        params['portfolio'] = {"enabled": True, "deadline_ms": time_budget, "candidate_count": 4}
        endpoint = "/v1/optimize"
    elif algorithm == 'beam':
        params['beam'] = {"enabled": True, "deadline_ms": time_budget, "beam_width": 2, "beam_depth": 2, "branch_factor": 2}
        endpoint = "/v1/optimize/beam"
    elif algorithm == 'alns':
        params['alns'] = {"enabled": True, "deadline_ms": time_budget, "iterations": 24, "segment_size": 6}
        endpoint = "/v1/optimize/alns"
    else:
        endpoint = "/v1/optimize"

    return endpoint, payload

def run_request(endpoint, payload, timeout_s=30):
    """Send request and return response body or None."""
    try:
        resp = requests.post(f"{BASE_URL}{endpoint}", json=payload, timeout=timeout_s)
        if resp.status_code == 200:
            return resp.json()
        else:
            return {"_status": resp.status_code, "_body": resp.text[:500]}
    except Exception as e:
        return {"_error": str(e)}

def main():
    # Generate seeds
    seeds = list(range(1001, 1001 + SEEDS_COUNT))

    # Configurations to test
    configs = []
    for fixture_name, fixture_path in FIXTURES.items():
        fixture = load_fixture(fixture_path)
        for layout_mode in ["guillotine", "nested"]:
            preset = PRESET_GUILLOTINE if layout_mode == "guillotine" else PRESET_NESTED
            time_budgets = TIME_BUDGETS[fixture_name]
            for algo in ALGORITHMS:
                time_budget = time_budgets[algo]
                configs.append({
                    'fixture_name': fixture_name,
                    'fixture': fixture,
                    'layout_mode': layout_mode,
                    'algorithm': algo,
                    'time_budget': time_budget,
                    'default_profile': DEFAULT_PROFILE,
                    'preset_profile': preset,
                })

    print(f"=" * 80)
    print(f"PRESET VALIDATION: {len(configs)} configurations x {SEEDS_COUNT} seeds")
    print(f"=" * 80)

    # Check service health
    try:
        resp = requests.get(f"{BASE_URL}/health/live", timeout=5)
        print(f"Service health: {resp.status_code}")
    except:
        print(f"ERROR: Cannot reach service at {BASE_URL}")
        print("Start with: cargo run --release")
        sys.exit(1)

    all_results = []

    for cfg_idx, cfg in enumerate(configs):
        label = f"{cfg['fixture_name']}/{cfg['layout_mode']}/{cfg['algorithm']}"
        print(f"\n{'='*60}")
        print(f"[{cfg_idx+1}/{len(configs)}] {label} (time_budget={cfg['time_budget']}ms)")
        print(f"{'='*60}")

        for profile_name, profile in [("default", cfg['default_profile']), ("preset", cfg['preset_profile'])]:
            out_dir = OUT_ROOT / label / profile_name
            out_dir.mkdir(parents=True, exist_ok=True)

            metrics_list = []
            ok_count = 0
            hard_ok_count = 0

            print(f"\n  --- Profile: {profile_name} ---")
            for i, seed in enumerate(seeds):
                endpoint, payload = build_request(
                    cfg['fixture'], cfg['layout_mode'], cfg['algorithm'],
                    profile, seed, cfg['time_budget']
                )
                timeout_s = max(15, cfg['time_budget'] / 1000.0 + 10)
                body = run_request(endpoint, payload, timeout_s=timeout_s)

                if body and body.get('status') == 'ok':
                    ok_count += 1
                    q = compute_quality_metrics(body)
                    if q['hard_ok']:
                        hard_ok_count += 1
                    metrics_list.append({**q, 'seed': seed, 'svg': body.get('artifacts', {}).get('svg', '')})

                    status_str = "OK" if q['hard_ok'] else "ok*"
                    print(f"    seed={seed}: {status_str} waste={q['waste_percent']:.2f}% "
                          f"sheets={q['used_sheets']} perim={q['occupied_perimeter']:.0f} "
                          f"vcomp={q['void_compactness']:.3f} corr_comp={q['corridor_components']}")
                else:
                    status = body.get('_status', '?') if body else 'ERR'
                    print(f"    seed={seed}: FAILED (status={status})")

            # Sort and save top-5
            hard_ok_list = [m for m in metrics_list if m['hard_ok']]
            hard_ok_list.sort(key=lambda m: m['sort_key'])
            top5 = hard_ok_list[:5] if hard_ok_list else sorted(metrics_list, key=lambda m: m['sort_key'])[:5]

            # Save SVGs
            for rank, m in enumerate(top5, 1):
                svg_name = f"rank_{rank:02d}_seed_{m['seed']}_waste_{m['waste_percent']:.2f}_perim_{m['occupied_perimeter']:.0f}.svg"
                svg_path = out_dir / svg_name
                with open(svg_path, 'w') as f:
                    f.write(m.get('svg', ''))

            # Save manifest
            manifest = {
                'profile': profile_name,
                'fixture': cfg['fixture_name'],
                'layout_mode': cfg['layout_mode'],
                'algorithm': cfg['algorithm'],
                'time_budget_ms': cfg['time_budget'],
                'seeds_tested': len(seeds),
                'ok_count': ok_count,
                'hard_ok_count': hard_ok_count,
                'profile_params': {k: v for k, v in profile.items() if k not in ('ga_profile', 'sla_profile')},
                'top5': [{k: v for k, v in m.items() if k != 'svg'} for m in top5],
            }
            with open(out_dir / 'manifest.json', 'w') as f:
                json.dump(manifest, f, indent=2, default=str)

            print(f"\n  {profile_name}: ok={ok_count}/{len(seeds)}, hard_ok={hard_ok_count}/{len(seeds)}")
            if top5:
                best = top5[0]
                print(f"  Best: waste={best['waste_percent']:.2f}% perim={best['occupied_perimeter']:.0f} "
                      f"vcomp={best['void_compactness']:.3f}")

            all_results.append({
                'config': label,
                'profile': profile_name,
                'ok': ok_count,
                'hard_ok': hard_ok_count,
                'top5_metrics': [{k: v for k, v in m.items() if k != 'svg'} for m in top5],
            })

    # Final comparison
    print(f"\n\n{'='*80}")
    print("COMPARISON: DEFAULT vs PRESET")
    print(f"{'='*80}")
    print(f"{'Config':<45} {'Profile':<10} {'ok':<5} {'hard_ok':<8} {'Best waste':<12} {'Best perim':<12}")
    print("-" * 92)

    for r in all_results:
        best_waste = r['top5_metrics'][0]['waste_percent'] if r['top5_metrics'] else '-'
        best_perim = f"{r['top5_metrics'][0]['occupied_perimeter']:.0f}" if r['top5_metrics'] else '-'
        print(f"{r['config']:<45} {r['profile']:<10} {r['ok']:<5} {r['hard_ok']:<8} {best_waste:<12} {best_perim:<12}")

    # Save summary
    summary_path = OUT_ROOT / 'summary.json'
    with open(summary_path, 'w') as f:
        json.dump(all_results, f, indent=2, default=str)
    print(f"\nSummary saved to: {summary_path}")
    print(f"SVG outputs in: {OUT_ROOT}/")

if __name__ == '__main__':
    main()
