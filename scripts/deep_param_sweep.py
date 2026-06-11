#!/usr/bin/env python3
"""
Deep Parameter Sweep for Cutting Quality Optimization.
Phase A: One-At-A-Time (OAT) sensitivity analysis
Phase B: Full factorial on top-2 values per parameter
Phase C: Cross-mode validation (nested + portfolio)
"""
import copy, json, math, os, sys, time, statistics
from collections import deque
from pathlib import Path
from itertools import product

import requests

BASE_URL = os.getenv("FREECUT_API_URL", "http://127.0.0.1:8088")
OUT_ROOT = Path("ai_docs/tmp/deep_sweep")
FIXTURE_PATH = "tests/fixtures/multisheet_varied_4sheets.json"
SEEDS = list(range(3001, 3011))  # 10 seeds
TIME_LIMIT_MS = 6000

# Parameter sweep ranges
SWEEP = {
    'epochs':          [60, 120, 180, 300, 500],
    'breed_factor':    [0.4, 0.5, 0.6, 0.7, 0.8],
    'survival_factor': [0.5, 0.6, 0.7, 0.8, 0.9],
    'top_k_candidates':[3, 6, 12, 24, 48],
}

# Baseline (balanced profile defaults)
BASELINE = {
    'epochs': 100, 'breed_factor': 0.5,
    'survival_factor': 0.6, 'top_k_candidates': 6,
}

# ============================================================================
# METRICS (reused from preset_validation_v2.py + new)
# ============================================================================
def build_occupancy_grid(placements, usable_w, usable_h, grid_mm):
    cols = max(1, int(math.ceil(usable_w / grid_mm)))
    rows = max(1, int(math.ceil(usable_h / grid_mm)))
    grid = [bytearray(cols) for _ in range(rows)]
    for p in placements:
        x0, y0 = p['x_mm'], p['y_mm']
        x1 = p['x_mm'] + p['width_mm']
        y1 = p['y_mm'] + p['height_mm']
        if x1 <= 0 or y1 <= 0 or x0 >= usable_w or y0 >= usable_h: continue
        gx0 = max(0, int(math.floor(x0/grid_mm)))
        gy0 = max(0, int(math.floor(y0/grid_mm)))
        gx1 = min(cols-1, int(math.ceil(x1/grid_mm))-1)
        gy1 = min(rows-1, int(math.ceil(y1/grid_mm))-1)
        if gx1 < gx0 or gy1 < gy0: continue
        fill = b'\x01' * (gx1 - gx0 + 1)
        for gy in range(gy0, gy1+1): grid[gy][gx0:gx1+1] = fill
    return grid, rows, cols

def flood_external_voids(grid, rows, cols):
    q = deque()
    def enq(r,c):
        if grid[r][c]==0: grid[r][c]=2; q.append((r,c))
    for c in range(cols):
        enq(0,c)
        if rows>1: enq(rows-1,c)
    for r in range(rows):
        enq(r,0)
        if cols>1: enq(r,cols-1)
    while q:
        r,c = q.popleft()
        for nr,nc in [(r-1,c),(r+1,c),(r,c-1),(r,c+1)]:
            if 0<=nr<rows and 0<=nc<cols: enq(nr,nc)

def compute_quality(body, grid_mm=5.0):
    solutions = body.get('solutions', [])
    total_internal = 0.0; total_perim = 0.0; total_void_cells = 0; total_void_perim = 0
    total_corr_comp = 0
    all_pieces = []  # for new metrics
    for sol in solutions:
        pl = sol.get('placements', [])
        if not pl: continue
        trim = sol.get('trim_mm') or {}
        uw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
        uh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
        if uw <= 0 or uh <= 0: continue
        # Collect piece data for new metrics
        for p in pl:
            all_pieces.append({
                'x': p['x_mm'], 'y': p['y_mm'],
                'w': p['width_mm'], 'h': p['height_mm'],
                'area': p['width_mm'] * p['height_mm'],
            })
        grid, rows, cols = build_occupancy_grid(pl, uw, uh, grid_mm)
        flood_external_voids(grid, rows, cols)
        # Internal voids
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    total_internal += grid_mm*grid_mm
                    grid[r][c] = 3
                    q = deque([(r,c)])
                    while q:
                        cr,cc = q.popleft()
                        for nr,nc in [(cr-1,cc),(cr+1,cc),(cr,cc-1),(cr,cc+1)]:
                            if 0<=nr<rows and 0<=nc<cols and grid[nr][nc]==0:
                                grid[nr][nc]=3; q.append((nr,nc))
        # Corridor components
        bmin_x = min(p['x_mm'] for p in pl); bmin_y = min(p['y_mm'] for p in pl)
        bmax_x = max(p['x_mm']+p['width_mm'] for p in pl)
        bmax_y = max(p['y_mm']+p['height_mm'] for p in pl)
        bx0 = max(0,int(math.floor(bmin_x/grid_mm)))
        by0 = max(0,int(math.floor(bmin_y/grid_mm)))
        bx1 = min(cols-1, int(math.ceil(bmax_x/grid_mm))-1)
        by1 = min(rows-1, int(math.ceil(bmax_y/grid_mm))-1)
        for r in range(by0, by1+1):
            for c in range(bx0, bx1+1):
                if grid[r][c] == 2:
                    total_corr_comp += 1; grid[r][c] = 4
                    q = deque([(r,c)])
                    while q:
                        cr,cc = q.popleft()
                        for nr,nc in [(cr-1,cc),(cr+1,cc),(cr,cc-1),(cr,cc+1)]:
                            if by0<=nr<=by1 and bx0<=nc<=bx1 and grid[nr][nc]==2:
                                grid[nr][nc]=4; q.append((nr,nc))
        # Perimeter
        for r in range(rows):
            for c in range(cols):
                if grid[r][c]==1:
                    if r==0 or grid[r-1][c]!=1: total_perim += grid_mm
                    if r==rows-1 or grid[r+1][c]!=1: total_perim += grid_mm
                    if c==0 or grid[r][c-1]!=1: total_perim += grid_mm
                    if c==cols-1 or grid[r][c+1]!=1: total_perim += grid_mm
        # Void compactness
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] != 1:
                    total_void_cells += 1
                    if r>0 and grid[r-1][c]==1: total_void_perim += 1
                    if r<rows-1 and grid[r+1][c]==1: total_void_perim += 1
                    if c>0 and grid[r][c-1]==1: total_void_perim += 1
                    if c<cols-1 and grid[r][c+1]==1: total_void_perim += 1

    vcomp = total_void_perim / total_void_cells if total_void_cells > 0 else 0.0
    waste = body.get('summary',{}).get('waste_percent', 999)
    sheets = body.get('summary',{}).get('used_stock_count', 99)

    # New metrics: isolated small pieces
    isolated_small = 0
    edge_fill_ratio = 0.0
    if all_pieces:
        areas = [p['area'] for p in all_pieces]
        median_area = statistics.median(areas)
        for p in all_pieces:
            if p['area'] < median_area:
                # Check if it shares an edge with a piece > 2x its area
                has_big_neighbor = False
                px0, py0, px1, py1 = p['x'], p['y'], p['x']+p['w'], p['y']+p['h']
                for o in all_pieces:
                    if o is p: continue
                    if o['area'] > 2 * p['area']:
                        ox0, oy0 = o['x'], o['y']
                        ox1, oy1 = o['x']+o['w'], o['y']+o['h']
                        # Check edge adjacency (within 10mm tolerance = kerf + spacing)
                        tol = 10.0
                        h_adj = (abs(px0-ox1)<tol or abs(px1-ox0)<tol) and not (py1<oy0 or py0>oy1)
                        v_adj = (abs(py0-oy1)<tol or abs(py1-oy0)<tol) and not (px1<ox0 or px0>ox1)
                        if h_adj or v_adj:
                            has_big_neighbor = True
                            break
                if not has_big_neighbor:
                    isolated_small += 1

        # Edge fill ratio: how much of the 4 sheet edges is covered by pieces
        piece_idx = 0
        for sol in solutions:
            pl = sol.get('placements', [])
            if not pl: continue
            trim = sol.get('trim_mm') or {}
            sw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
            sh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
            if sw <= 0 or sh <= 0: continue
            tol = 5.0
            sol_pieces = all_pieces[piece_idx:piece_idx+len(pl)]
            piece_idx += len(pl)
            # Bottom edge (y near 0)
            bottom_covered = sum(p['w'] for p in sol_pieces if p['y'] < tol)
            # Top edge (y+h near sh)
            top_covered = sum(p['w'] for p in sol_pieces if abs((p['y']+p['h'])-sh) < tol)
            # Left edge (x near 0)
            left_covered = sum(p['h'] for p in sol_pieces if p['x'] < tol)
            # Right edge (x+w near sw)
            right_covered = sum(p['h'] for p in sol_pieces if abs((p['x']+p['w'])-sw) < tol)
            edge_total = 2*sw + 2*sh
            filled = min(bottom_covered, sw) + min(top_covered, sw) + min(left_covered, sh) + min(right_covered, sh)
            edge_fill_ratio = filled / edge_total if edge_total > 0 else 0.0

    unplaced = body.get('unplaced_items', [])
    placed = sum(len(s.get('placements',[])) for s in solutions)
    pu = sum(1 for it in unplaced if it.get('reason')!='oversized')
    tp = placed + pu
    pr = placed/tp if tp>0 else 1.0
    hard_ok = pr == 1.0 and total_internal == 0.0
    return {
        'internal_void': total_internal, 'perimeter': total_perim,
        'vcomp': round(vcomp, 4), 'corr_comp': total_corr_comp,
        'waste': waste, 'sheets': sheets, 'hard_ok': hard_ok,
        'placeable_ratio': pr, 'isolated_small': isolated_small,
        'edge_fill': round(edge_fill_ratio, 4),
        'sort_key': (sheets, total_internal, total_perim, -edge_fill_ratio, total_corr_comp),
    }

# ============================================================================
# HTTP HELPERS
# ============================================================================
def make_request(seed, layout_mode='guillotine', algorithm='standard', ga_override=None):
    with open(FIXTURE_PATH) as f:
        req_body = json.load(f)
    req_body['params']['seed'] = seed
    req_body['params']['time_limit_ms'] = TIME_LIMIT_MS
    req_body['params']['layout_mode'] = layout_mode
    if algorithm == 'portfolio':
        req_body['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': TIME_LIMIT_MS}
    if ga_override:
        req_body['params']['ga_profile'] = 'balanced'
        req_body['params']['ga_override'] = ga_override
    return req_body

def call_optimize(req_body, timeout=30):
    try:
        r = requests.post(f"{BASE_URL}/v1/optimize", json=req_body, timeout=timeout)
        if r.status_code == 200:
            return r.json()
        return None
    except Exception:
        return None

# ============================================================================
# PHASE RUNNER
# ============================================================================
def run_config_batch(config_name, layout_mode, algorithm, ga_override, save_svg=True):
    """Run a config across all seeds, return per-seed metrics."""
    results = []
    svg_saved = 0
    for seed in SEEDS:
        body = make_request(seed, layout_mode, algorithm, ga_override)
        resp = call_optimize(body)
        if resp is None:
            results.append({'seed': seed, 'error': True})
            continue
        q = compute_quality(resp)
        q['seed'] = seed
        q['error'] = False
        results.append(q)
        if save_svg and q.get('hard_ok') and svg_saved < 3:
            svg = resp.get('svg')
            if svg:
                svg_path = OUT_ROOT / "svgs" / f"{config_name}_seed{seed}_w{q['waste']:.1f}_P{q['perimeter']:.0f}.svg"
                svg_path.parent.mkdir(parents=True, exist_ok=True)
                svg_path.write_text(svg, encoding='utf-8')
                svg_saved += 1
    return results

def summarize_batch(results):
    """Aggregate metrics across seeds."""
    ok = [r for r in results if not r.get('error')]
    if not ok:
        return {'count': 0, 'hard_ok_count': 0}
    hard_oks = [r for r in ok if r.get('hard_ok')]
    return {
        'count': len(ok),
        'hard_ok_count': len(hard_oks),
        'hard_ok_rate': len(hard_oks)/len(ok),
        'avg_sheets': statistics.mean([r['sheets'] for r in ok]),
        'min_sheets': min(r['sheets'] for r in ok),
        'avg_perimeter': statistics.mean([r['perimeter'] for r in ok]),
        'min_perimeter': min(r['perimeter'] for r in ok),
        'avg_vcomp': statistics.mean([r['vcomp'] for r in ok]),
        'avg_corr_comp': statistics.mean([r['corr_comp'] for r in ok]),
        'avg_isolated_small': statistics.mean([r['isolated_small'] for r in ok]),
        'avg_edge_fill': statistics.mean([r['edge_fill'] for r in ok]),
        'avg_waste': statistics.mean([r['waste'] for r in ok if isinstance(r['waste'], (int, float))]),
    }

# ============================================================================
# PHASE A: One-At-A-Time Sensitivity
# ============================================================================
def phase_a():
    print("=" * 70)
    print("PHASE A: One-At-A-Time Sensitivity Analysis")
    print("=" * 70)
    oat_results = {}  # {param_name: {value: summary}}

    for param_name, values in SWEEP.items():
        print(f"\n  Sweeping: {param_name} (baseline: {BASELINE})")
        oat_results[param_name] = {}
        for val in values:
            ga = dict(BASELINE)
            ga[param_name] = val
            config_name = f"A_{param_name}_{val}"
            print(f"    {param_name}={val} ...", end="", flush=True)
            results = run_config_batch(config_name, 'guillotine', 'standard', ga, save_svg=False)
            summary = summarize_batch(results)
            oat_results[param_name][val] = summary
            print(f" hard_ok={summary.get('hard_ok_count',0)}/10, "
                  f"sheets={summary.get('avg_sheets',0):.1f}, "
                  f"perim={summary.get('avg_perimeter',0):.0f}, "
                  f"isol={summary.get('avg_isolated_small',0):.1f}")
        # Find top-2 values by composite score
        scored = []
        for val in values:
            s = oat_results[param_name][val]
            if s.get('count', 0) == 0: continue
            # Composite: lower is better
            score = (s['avg_sheets'] * 1000
                     + s['avg_perimeter']
                     + s['avg_isolated_small'] * 200
                     - s['avg_edge_fill'] * 500
                     + s['avg_corr_comp'] * 100
                     - s['hard_ok_rate'] * 2000)
            scored.append((score, val))
        scored.sort()
        top2 = [s[1] for s in scored[:2]]
        oat_results[param_name]['_top2'] = top2
        print(f"    => Top-2 for {param_name}: {top2}")

    # Print sensitivity table
    print("\n" + "=" * 70)
    print("SENSITIVITY TABLE (Phase A)")
    print("=" * 70)
    for param_name, values in SWEEP.items():
        print(f"\n  {param_name}:")
        print(f"    {'Value':>8} | {'hard_ok':>7} | {'sheets':>6} | {'perim':>8} | {'isol':>5} | {'edge_fill':>9} | {'corr':>5}")
        print(f"    {'-'*8}-+-{'-'*7}-+-{'-'*6}-+-{'-'*8}-+-{'-'*5}-+-{'-'*9}-+-{'-'*5}")
        for val in values:
            s = oat_results[param_name][val]
            print(f"    {str(val):>8} | {s.get('hard_ok_count',0):>4}/10 | "
                  f"{s.get('avg_sheets',0):>6.2f} | {s.get('avg_perimeter',0):>8.0f} | "
                  f"{s.get('avg_isolated_small',0):>5.1f} | {s.get('avg_edge_fill',0):>9.4f} | "
                  f"{s.get('avg_corr_comp',0):>5.1f}")

    return oat_results

# ============================================================================
# PHASE B: Full Factorial on Top-2
# ============================================================================
def phase_b(oat_results):
    print("\n" + "=" * 70)
    print("PHASE B: Full Factorial Interaction Analysis")
    print("=" * 70)
    # Get top-2 per param
    param_vals = {}
    for param_name in SWEEP:
        top2 = oat_results[param_name].get('_top2', SWEEP[param_name][:2])
        param_vals[param_name] = top2
        print(f"  {param_name}: {top2}")

    # Generate all combinations
    keys = list(param_vals.keys())
    combos = list(product(*[param_vals[k] for k in keys]))
    print(f"\n  Running {len(combos)} configurations x {len(SEEDS)} seeds = {len(combos)*len(SEEDS)} requests")

    factorial_results = {}
    for i, combo in enumerate(combos):
        ga = {k: v for k, v in zip(keys, combo)}
        config_name = "B_" + "_".join(f"{k[:3]}{v}" for k,v in zip(keys, combo))
        print(f"  [{i+1}/{len(combos)}] {config_name} ...", end="", flush=True)
        results = run_config_batch(config_name, 'guillotine', 'standard', ga, save_svg=True)
        summary = summarize_batch(results)
        factorial_results[config_name] = {'ga': ga, 'summary': summary, 'results': results}
        print(f" hard_ok={summary.get('hard_ok_count',0)}/10, "
              f"sheets={summary.get('avg_sheets',0):.1f}, "
              f"perim={summary.get('avg_perimeter',0):.0f}, "
              f"isol={summary.get('avg_isolated_small',0):.1f}")

    # Rank by composite score
    ranked = []
    for name, data in factorial_results.items():
        s = data['summary']
        if s.get('count', 0) == 0: continue
        score = (s['avg_sheets'] * 1000
                 + s['avg_perimeter']
                 + s['avg_isolated_small'] * 200
                 - s['avg_edge_fill'] * 500
                 + s['avg_corr_comp'] * 100
                 - s['hard_ok_rate'] * 2000)
        ranked.append((score, name, data))
    ranked.sort()

    print("\n" + "=" * 70)
    print("PHASE B RANKING (top 10):")
    print("=" * 70)
    print(f"  {'Rank':>4} | {'Config':>40} | {'hard_ok':>7} | {'sheets':>6} | {'perim':>8} | {'isol':>5} | {'edge':>6}")
    for rank, (score, name, data) in enumerate(ranked[:10], 1):
        s = data['summary']
        print(f"  {rank:>4} | {name:>40} | {s.get('hard_ok_count',0):>4}/10 | "
              f"{s.get('avg_sheets',0):>6.2f} | {s.get('avg_perimeter',0):>8.0f} | "
              f"{s.get('avg_isolated_small',0):>5.1f} | {s.get('avg_edge_fill',0):>6.4f}")

    best_ga = ranked[0][2]['ga'] if ranked else dict(BASELINE)
    print(f"\n  Best GA config: {best_ga}")
    return factorial_results, best_ga, ranked

# ============================================================================
# PHASE C: Cross-Mode Validation
# ============================================================================
def phase_c(best_ga):
    print("\n" + "=" * 70)
    print("PHASE C: Cross-Mode Validation")
    print("=" * 70)
    modes = [
        ('guillotine', 'standard'),
        ('guillotine', 'portfolio'),
        ('nested', 'standard'),
        ('nested', 'portfolio'),
    ]
    cross_results = {}
    for layout, algo in modes:
        config_name = f"C_{layout}_{algo}"
        print(f"  {config_name} ...", end="", flush=True)
        results = run_config_batch(config_name, layout, algo, best_ga, save_svg=True)
        summary = summarize_batch(results)
        cross_results[config_name] = {'layout': layout, 'algo': algo, 'summary': summary, 'results': results}
        print(f" hard_ok={summary.get('hard_ok_count',0)}/10, "
              f"sheets={summary.get('avg_sheets',0):.1f}, "
              f"perim={summary.get('avg_perimeter',0):.0f}, "
              f"isol={summary.get('avg_isolated_small',0):.1f}")

    print("\n" + "=" * 70)
    print("PHASE C CROSS-MODE COMPARISON:")
    print("=" * 70)
    print(f"  {'Mode':>30} | {'hard_ok':>7} | {'sheets':>6} | {'perim':>8} | {'isol':>5} | {'edge':>6} | {'corr':>5}")
    for name, data in cross_results.items():
        s = data['summary']
        print(f"  {name:>30} | {s.get('hard_ok_count',0):>4}/10 | "
              f"{s.get('avg_sheets',0):>6.2f} | {s.get('avg_perimeter',0):>8.0f} | "
              f"{s.get('avg_isolated_small',0):>5.1f} | {s.get('avg_edge_fill',0):>6.4f} | "
              f"{s.get('avg_corr_comp',0):>5.1f}")
    return cross_results

# ============================================================================
# MAIN
# ============================================================================
def main():
    OUT_ROOT.mkdir(parents=True, exist_ok=True)
    print("Deep Parameter Sweep for Cutting Quality")
    print(f"Service: {BASE_URL}")
    print(f"Fixture: {FIXTURE_PATH}")
    print(f"Seeds: {SEEDS[0]}-{SEEDS[-1]} ({len(SEEDS)} per config)")
    print(f"Time limit: {TIME_LIMIT_MS}ms")
    start = time.time()

    # Phase A
    oat_results = phase_a()
    phase_a_time = time.time() - start

    # Phase B
    factorial_results, best_ga, ranked_b = phase_b(oat_results)
    phase_b_time = time.time() - start - phase_a_time

    # Phase C
    cross_results = phase_c(best_ga)
    phase_c_time = time.time() - start - phase_a_time - phase_b_time

    # Save summary
    total_time = time.time() - start
    summary = {
        'total_time_s': round(total_time, 1),
        'phase_times': {
            'A_sensitivity': round(phase_a_time, 1),
            'B_factorial': round(phase_b_time, 1),
            'C_cross_mode': round(phase_c_time, 1),
        },
        'best_ga_config': best_ga,
        'phase_a_oat': {p: {str(k): v for k, v in vals.items()} for p, vals in oat_results.items()},
        'phase_b_factorial': {n: {'ga': d['ga'], 'summary': d['summary']} for n, d in factorial_results.items()},
        'phase_b_ranking': [{'rank': i+1, 'config': n, 'ga': d['ga'], 'summary': d['summary']} for i, (_, n, d) in enumerate(ranked_b[:10])],
        'phase_c_cross_mode': {n: {'layout': d['layout'], 'algo': d['algo'], 'summary': d['summary']} for n, d in cross_results.items()},
    }
    summary_path = OUT_ROOT / "summary.json"
    summary_path.write_text(json.dumps(summary, indent=2), encoding='utf-8')
    print(f"\nResults saved to {summary_path}")
    print(f"Total time: {total_time:.0f}s")

    # Final recommendation
    print("\n" + "=" * 70)
    print("FINAL RECOMMENDATION")
    print("=" * 70)
    print(f"  Best GA params: {best_ga}")
    best_cross = min(cross_results.items(), key=lambda x: x[1]['summary'].get('avg_sheets', 99))
    print(f"  Best mode: {best_cross[0]} (sheets={best_cross[1]['summary'].get('avg_sheets',0):.1f})")
    print(f"\n  Parameter sensitivity (by impact on sheets):")
    for param_name in SWEEP:
        values = SWEEP[param_name]
        sheet_vals = [oat_results[param_name][v].get('avg_sheets', 99) for v in values]
        spread = max(sheet_vals) - min(sheet_vals)
        print(f"    {param_name}: spread={spread:.2f} sheets (values: {[f'{v:.1f}' for v in sheet_vals]})")

if __name__ == "__main__":
    main()
