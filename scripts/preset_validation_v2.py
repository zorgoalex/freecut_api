#!/usr/bin/env python3
"""
Preset Validation Script v2
Tests ACTUALLY IMPLEMENTED parameters that affect cutting quality:
  - layout_mode (guillotine vs nested)
  - ga_profile (fast/balanced/quality)
  - sla_profile (fast/balanced/quality)
  - Algorithm mode (standard/portfolio/beam/alns)
  - time_limit_ms budgets
  - seed variation
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

BASE_URL = os.getenv("FREECUT_API_URL", "http://127.0.0.1:8088")
SEEDS_COUNT = 20
OUT_ROOT = Path("ai_docs/tmp/preset_validation_v2")

FIXTURES = {
    "oversized": "tests/fixtures/multisheet_oversized.json",
    "varied_4sheets": "tests/fixtures/multisheet_varied_4sheets.json",
}

# Test matrix: each row = one configuration to test
PROFILES = [
    {"name": "default",         "ga_profile": "balanced", "sla_profile": "balanced"},
    {"name": "quality_ga",      "ga_profile": "quality",  "sla_profile": "balanced"},
    {"name": "full_quality",    "ga_profile": "quality",  "sla_profile": "quality"},
]

# Algorithms with time budgets per fixture
ALGORITHMS_TO_TEST = ["standard", "portfolio"]

ALGO_CONFIGS = {
    "oversized": {
        "standard":  {"time_ms": 2000,  "endpoint": "/v1/optimize"},
        "portfolio": {"time_ms": 3000,  "endpoint": "/v1/optimize"},
    },
    "varied_4sheets": {
        "standard":  {"time_ms": 6000,  "endpoint": "/v1/optimize"},
        "portfolio": {"time_ms": 6000,  "endpoint": "/v1/optimize"},
    },
}

LAYOUT_MODES = ["guillotine", "nested"]

# ============================================================================
# METRICS
# ============================================================================
def build_occupancy_grid(placements, usable_w, usable_h, grid_mm, pad_mm):
    cols = max(1, int(math.ceil(usable_w / grid_mm)))
    rows = max(1, int(math.ceil(usable_h / grid_mm)))
    grid = [bytearray(cols) for _ in range(rows)]
    for p in placements:
        x0, y0 = p['x_mm'] - pad_mm, p['y_mm'] - pad_mm
        x1 = p['x_mm'] + p['width_mm'] + pad_mm
        y1 = p['y_mm'] + p['height_mm'] + pad_mm
        if x1 <= 0 or y1 <= 0 or x0 >= usable_w or y0 >= usable_h: continue
        gx0, gy0 = max(0, int(math.floor(x0/grid_mm))), max(0, int(math.floor(y0/grid_mm)))
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

def compute_quality(body, grid_mm=5.0, spacing_mm=0.5):
    solutions = body.get('solutions', [])
    total_internal = 0.0; total_perim = 0.0; total_void_cells = 0; total_void_perim = 0
    total_corr_comp = 0
    for sol in solutions:
        pl = sol.get('placements', [])
        if not pl: continue
        trim = sol.get('trim_mm') or {}
        uw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
        uh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
        if uw <= 0 or uh <= 0: continue
        grid, rows, cols = build_occupancy_grid(pl, uw, uh, grid_mm, 0)
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
        # Corridor components (external void inside bbox)
        bmin_x = min(p['x_mm'] for p in pl); bmin_y = min(p['y_mm'] for p in pl)
        bmax_x = max(p['x_mm']+p['width_mm'] for p in pl); bmax_y = max(p['y_mm']+p['height_mm'] for p in pl)
        bx0,by0 = max(0,int(math.floor(bmin_x/grid_mm))), max(0,int(math.floor(bmin_y/grid_mm)))
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
        # Occupied perimeter
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
    unplaced = body.get('unplaced_items', [])
    placed = sum(len(s.get('placements',[])) for s in solutions)
    pu = sum(1 for it in unplaced if it.get('reason')!='oversized')
    tp = placed + pu
    pr = placed/tp if tp>0 else 1.0
    hard_ok = pr == 1.0 and total_internal == 0.0
    return {
        'internal_void': total_internal, 'perimeter': total_perim,
        'vcomp': vcomp, 'corr_comp': total_corr_comp,
        'waste': waste, 'sheets': sheets, 'hard_ok': hard_ok,
        'placeable_ratio': pr,
        'sort_key': (total_internal, total_perim, vcomp, total_corr_comp, waste),
    }

def load_fixture(path):
    with open(path) as f: return json.load(f)

def build_request(fixture, layout_mode, algo, profile, seed, time_ms):
    payload = copy.deepcopy(fixture)
    p = payload.setdefault('params', {})
    p['layout_mode'] = layout_mode
    p['seed'] = seed
    p['restarts'] = 2
    p['include_svg'] = True
    p['time_limit_ms'] = time_ms
    p['ga_profile'] = profile['ga_profile']
    p['sla_profile'] = profile['sla_profile']
    # Clear algo-specific
    p.pop('portfolio', None); p.pop('beam', None); p.pop('alns', None)
    endpoint = ALGO_CONFIGS.get(fixture.get('_fixture_name',''),{}).get(algo,{}).get('endpoint','/v1/optimize')
    if algo == 'portfolio':
        p['portfolio'] = {"enabled": True, "deadline_ms": time_ms, "candidate_count": 4}
        endpoint = "/v1/optimize"
    elif algo == 'beam':
        p['beam'] = {"enabled": True, "deadline_ms": time_ms, "beam_width": 2, "beam_depth": 2, "branch_factor": 2}
        endpoint = "/v1/optimize/beam"
    elif algo == 'alns':
        p['alns'] = {"enabled": True, "deadline_ms": time_ms, "iterations": 24, "segment_size": 6}
        endpoint = "/v1/optimize/alns"
    return endpoint, payload

def run_req(endpoint, payload, timeout_s=30):
    try:
        r = requests.post(f"{BASE_URL}{endpoint}", json=payload, timeout=timeout_s)
        return r.json() if r.status_code == 200 else {"_status": r.status_code}
    except Exception as e:
        return {"_error": str(e)}

def main():
    seeds = list(range(2001, 2001 + SEEDS_COUNT))

    # Build test matrix: fixture x layout_mode x algo x profile
    tests = []
    for fix_name, fix_path in FIXTURES.items():
        fix = load_fixture(fix_path)
        fix['_fixture_name'] = fix_name
        for lm in LAYOUT_MODES:
            for algo in ALGORITHMS_TO_TEST:
                if algo not in ALGO_CONFIGS[fix_name]: continue
                acfg = ALGO_CONFIGS[fix_name][algo]
                for prof in PROFILES:
                    tests.append({
                        'fix_name': fix_name, 'fix': fix, 'lm': lm,
                        'algo': algo, 'time_ms': acfg['time_ms'], 'profile': prof,
                    })

    print(f"{'='*80}")
    print(f"PRESET VALIDATION v2: {len(tests)} configs x {SEEDS_COUNT} seeds = {len(tests)*SEEDS_COUNT} requests")
    print(f"{'='*80}")

    try:
        r = requests.get(f"{BASE_URL}/health/live", timeout=5)
        print(f"Service: {r.status_code}")
    except:
        print(f"ERROR: Service not reachable at {BASE_URL}"); sys.exit(1)

    all_results = []

    for ti, t in enumerate(tests):
        label = f"{t['fix_name']}/{t['lm']}/{t['algo']}/{t['profile']['name']}"
        out_dir = OUT_ROOT / t['fix_name'] / t['lm'] / t['algo'] / t['profile']['name']
        out_dir.mkdir(parents=True, exist_ok=True)

        print(f"\n[{ti+1}/{len(tests)}] {label} (t={t['time_ms']}ms)")

        metrics_list = []
        ok_cnt = 0; hard_cnt = 0

        for seed in seeds:
            ep, payload = build_request(t['fix'], t['lm'], t['algo'], t['profile'], seed, t['time_ms'])
            timeout_s = max(15, t['time_ms']/1000 + 10)
            body = run_req(ep, payload, timeout_s)
            if body and body.get('status') == 'ok':
                ok_cnt += 1
                q = compute_quality(body)
                if q['hard_ok']: hard_cnt += 1
                metrics_list.append({**q, 'seed': seed, 'svg': body.get('artifacts',{}).get('svg','')})
            else:
                st = body.get('_status','?') if body else 'ERR'
                metrics_list.append({'seed': seed, 'failed': True, 'status': st})

        # Stats
        ok_list = [m for m in metrics_list if not m.get('failed')]
        hard_list = [m for m in ok_list if m.get('hard_ok')]
        hard_list_sorted = sorted(hard_list, key=lambda m: m['sort_key'])
        top5 = hard_list_sorted[:5] if hard_list_sorted else sorted(ok_list, key=lambda m: m.get('sort_key',(999,)))[:5]

        # Save top-5 SVGs
        for rank, m in enumerate(top5, 1):
            svg_path = out_dir / f"rank_{rank:02d}_seed_{m['seed']}_w{m.get('waste',0):.1f}_p{m.get('perimeter',0):.0f}.svg"
            with open(svg_path, 'w') as f: f.write(m.get('svg',''))

        # Stats summary
        if ok_list:
            wastes = [m['waste'] for m in ok_list if 'waste' in m]
            perims = [m['perimeter'] for m in ok_list if 'perimeter' in m]
            min_waste = min(wastes) if wastes else -1
            avg_perim = sum(perims)/len(perims) if perims else -1
            min_perim = min(perims) if perims else -1
            # Count unique perimeters (layout diversity)
            unique_perims = len(set(perims))
        else:
            min_waste = avg_perim = min_perim = -1; unique_perims = 0

        result = {
            'label': label, 'fix': t['fix_name'], 'lm': t['lm'], 'algo': t['algo'],
            'profile': t['profile']['name'], 'ga': t['profile']['ga_profile'],
            'sla': t['profile']['sla_profile'], 'time_ms': t['time_ms'],
            'ok': ok_cnt, 'hard_ok': hard_cnt,
            'min_waste': min_waste, 'avg_perim': avg_perim, 'min_perim': min_perim,
            'unique_layouts': unique_perims,
            'top5': [{k:v for k,v in m.items() if k != 'svg'} for m in top5],
        }
        all_results.append(result)

        # Manifest
        with open(out_dir / 'manifest.json', 'w') as f: json.dump(result, f, indent=2, default=str)

        print(f"  ok={ok_cnt} hard_ok={hard_cnt} min_waste={min_waste:.2f}% "
              f"min_perim={min_perim:.0f} avg_perim={avg_perim:.0f} unique={unique_perims}")

    # ============================================================
    # COMPARISON TABLE
    # ============================================================
    print(f"\n\n{'='*120}")
    print("FULL COMPARISON TABLE")
    print(f"{'='*120}")
    print(f"{'Fixture':<16} {'Mode':<10} {'Algo':<10} {'Profile':<16} {'ok':<4} {'hard':<5} {'minW%':<8} {'minP':<10} {'avgP':<10} {'uniq':<5}")
    print("-"*120)

    for r in all_results:
        print(f"{r['fix']:<16} {r['lm']:<10} {r['algo']:<10} {r['profile']:<16} "
              f"{r['ok']:<4} {r['hard_ok']:<5} {r['min_waste']:<8.2f} "
              f"{r['min_perim']:<10.0f} {r['avg_perim']:<10.0f} {r['unique_layouts']:<5}")

    # ============================================================
    # PER-FIXTURE BEST CONFIG
    # ============================================================
    print(f"\n\n{'='*80}")
    print("BEST CONFIGURATION PER FIXTURE (by avg_perim among hard_ok)")
    print(f"{'='*80}")

    for fix_name in FIXTURES:
        fix_results = [r for r in all_results if r['fix'] == fix_name and r['hard_ok'] > 0]
        if not fix_results:
            print(f"\n{fix_name}: No hard_ok results")
            continue
        fix_results.sort(key=lambda r: r['avg_perim'])
        best = fix_results[0]
        print(f"\n{fix_name}:")
        print(f"  Best: {best['lm']}/{best['algo']}/{best['profile']} "
              f"(avg_perim={best['avg_perim']:.0f}, min_waste={best['min_waste']:.2f}%, "
              f"hard_ok={best['hard_ok']}/{SEEDS_COUNT})")
        print(f"  Top-5 configs:")
        for i, r in enumerate(fix_results[:5], 1):
            print(f"    {i}. {r['lm']}/{r['algo']}/{r['profile']} "
                  f"avg_perim={r['avg_perim']:.0f} min_waste={r['min_waste']:.2f}% "
                  f"unique={r['unique_layouts']}")

    # Save summary
    summary_path = OUT_ROOT / 'summary.json'
    with open(summary_path, 'w') as f: json.dump(all_results, f, indent=2, default=str)
    print(f"\nSummary: {summary_path}")

if __name__ == '__main__':
    main()
