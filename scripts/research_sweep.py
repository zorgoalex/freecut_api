"""Research sweep: fragmentation penalty values + time budget + combination."""
import json, requests, copy, time, statistics, subprocess, os, sys

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"
SEEDS = 10

with open(FIXTURE) as f:
    base_req = json.load(f)

def compute_quality(data):
    summary = data.get('summary', {})
    solutions = data.get('solutions', [])
    unplaced = data.get('unplaced_items', [])
    cs = summary.get('candidate_selection', {})
    
    placed = sum(len(s.get('placements', [])) for s in solutions)
    unplaced_non_os = sum(1 for it in unplaced if it.get('reason') != 'oversized')
    tp = placed + unplaced_non_os
    pr = placed / tp if tp > 0 else 1.0
    
    sheets = summary.get('used_stock_count', 0)
    waste = summary.get('total_waste_area_mm2', 0)
    perim = cs.get('winner_piece_perimeter_mm', 0)
    bbox_void = cs.get('winner_bbox_void_area_mm2', 0)
    
    hard_ok = pr == 1.0 and bbox_void == 0.0
    return {'sheets': sheets, 'waste': waste, 'perim': perim,
            'bbox_void': bbox_void, 'placed': placed, 'unplaced': unplaced_non_os,
            'hard_ok': hard_ok}

def run_test(label, layout_mode, time_ms, use_portfolio=False, ga_override=None):
    results = []
    for seed in range(SEEDS):
        req = copy.deepcopy(base_req)
        req['params']['seed'] = seed
        req['params']['layout_mode'] = layout_mode
        req['params']['time_limit_ms'] = time_ms
        req['params']['restarts'] = 5
        if use_portfolio:
            req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': time_ms}
        elif 'portfolio' in req['params']:
            del req['params']['portfolio']
        if ga_override:
            req['params']['ga_override'] = ga_override
        
        try:
            r = requests.post(URL, json=req, timeout=120)
            if r.status_code != 200:
                continue
            q = compute_quality(r.json())
            results.append(q)
        except:
            continue
    
    if not results:
        return None
    
    sheets_list = [r['sheets'] for r in results]
    voids = [r['bbox_void'] for r in results]
    hard_oks = sum(1 for r in results if r['hard_ok'])
    
    return {
        'label': label,
        'n': len(results),
        'avg_sheets': statistics.mean(sheets_list),
        'min_sheets': min(sheets_list),
        '4sheet_rate': sheets_list.count(4) / len(results),
        'avg_void': statistics.mean(voids),
        'min_void': min(voids),
        'max_void': max(voids),
        'hard_ok': hard_oks,
        'sheets_dist': sorted(sheets_list),
    }

def print_result(r):
    if not r:
        print(f"  NO RESULTS")
        return
    print(f"  sheets={r['avg_sheets']:.1f} (4s:{r['4sheet_rate']:.0%}) "
          f"void={r['avg_void']/1e6:.2f}M [{r['min_void']/1e6:.2f}-{r['max_void']/1e6:.2f}] "
          f"hard_ok={r['hard_ok']}/{r['n']} dist={r['sheets_dist']}")

# ── Experiment 1: Time budget sweep with current fragmentation penalty (0.5) ──
print("=" * 80)
print("EXPERIMENT 1: Time budget sweep (fragmentation penalty=0.5, nested+portfolio)")
print("=" * 80)

for t_ms in [2000, 5000, 10000, 20000, 30000]:
    r = run_test(f"time={t_ms}", 'nested', t_ms, use_portfolio=True)
    print(f"  t={t_ms/1000:.0f}s:", end="")
    print_result(r)

# ── Experiment 2: Time budget with guillotine+standard ──
print("\n" + "=" * 80)
print("EXPERIMENT 2: Time budget sweep (fragmentation penalty=0.5, guillotine+standard)")
print("=" * 80)

ga_ov = {'epochs': 300, 'breed_factor': 0.5, 'survival_factor': 0.8, 'top_k_candidates': 3}
for t_ms in [2000, 5000, 10000, 20000, 30000]:
    r = run_test(f"time={t_ms}", 'guillotine', t_ms, ga_override=ga_ov)
    print(f"  t={t_ms/1000:.0f}s:", end="")
    print_result(r)

# ── Experiment 3: guillotine+portfolio (longer time) ──
print("\n" + "=" * 80)
print("EXPERIMENT 3: guillotine+portfolio (fragmentation=0.5)")
print("=" * 80)

for t_ms in [5000, 10000, 20000]:
    r = run_test(f"time={t_ms}", 'guillotine', t_ms, use_portfolio=True)
    print(f"  t={t_ms/1000:.0f}s:", end="")
    print_result(r)

# ── Experiment 4: nested+standard (longer time) ──
print("\n" + "=" * 80)
print("EXPERIMENT 4: nested+standard (fragmentation=0.5)")
print("=" * 80)

for t_ms in [5000, 10000, 20000]:
    r = run_test(f"time={t_ms}", 'nested', t_ms, ga_override=ga_ov)
    print(f"  t={t_ms/1000:.0f}s:", end="")
    print_result(r)

print("\n" + "=" * 80)
print("DONE")
print("=" * 80)
