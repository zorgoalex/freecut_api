"""Diagnostic: understand nested+portfolio behavior and hard_ok failures."""
import json, requests, copy, time

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"

with open(FIXTURE) as f:
    base_req = json.load(f)

def compute_quality(data):
    """Compute quality metrics including hard_ok components."""
    summary = data.get('summary', {})
    solutions = data.get('solutions', [])
    unplaced = data.get('unplaced_items', [])
    
    # Placement ratio
    placed = sum(len(s.get('placements', [])) for s in solutions)
    unplaced_non_oversized = sum(1 for it in unplaced if it.get('reason') != 'oversized')
    total_placeable = placed + unplaced_non_oversized
    pr = placed / total_placeable if total_placeable > 0 else 1.0
    
    # Internal voids
    cs = summary.get('candidate_selection', {})
    bbox_void = cs.get('winner_bbox_void_area_mm2', 0)
    
    # Sheets and waste
    sheets = summary.get('used_stock_count', 0)
    waste = summary.get('total_waste_area_mm2', 0)
    perim = cs.get('winner_piece_perimeter_mm', 0)
    
    hard_ok = pr == 1.0 and bbox_void == 0.0
    
    return {
        'sheets': sheets,
        'waste': waste,
        'perim': perim,
        'bbox_void': bbox_void,
        'placed': placed,
        'unplaced': unplaced_non_oversized,
        'placement_ratio': pr,
        'hard_ok': hard_ok,
    }

print("Testing nested+portfolio mode (10 seeds)")
print("=" * 70)

for seed in range(10):
    req = copy.deepcopy(base_req)
    req['params']['seed'] = seed
    req['params']['layout_mode'] = 'nested'
    req['params']['time_limit_ms'] = 5000
    req['params']['restarts'] = 5
    req['params']['portfolio'] = {
        'enabled': True,
        'candidate_count': 5,
        'deadline_ms': 5000
    }
    
    t0 = time.time()
    r = requests.post(URL, json=req, timeout=60)
    dt = time.time() - t0
    
    if r.status_code != 200:
        print(f"seed={seed}: ERROR {r.status_code}")
        continue
    
    data = r.json()
    q = compute_quality(data)
    
    status = "OK" if q['hard_ok'] else "FAIL"
    fail_reason = ""
    if not q['hard_ok']:
        if q['placement_ratio'] < 1.0:
            fail_reason = f" (unplaced={q['unplaced']})"
        elif q['bbox_void'] > 0:
            fail_reason = f" (void={q['bbox_void']:.0f}mm2)"
    
    print(f"seed={seed}: sheets={q['sheets']} waste={q['waste']:.0f} "
          f"perim={q['perim']:.0f} placed={q['placed']}/{q['placed']+q['unplaced']} "
          f"void={q['bbox_void']:.0f} [{status}]{fail_reason} {dt:.1f}s")

print("\n" + "=" * 70)
print("Testing guillotine+standard for comparison")
print("=" * 70)

for seed in range(10):
    req = copy.deepcopy(base_req)
    req['params']['seed'] = seed
    req['params']['layout_mode'] = 'guillotine'
    req['params']['time_limit_ms'] = 5000
    req['params']['restarts'] = 5
    req['params']['ga_override'] = {
        'epochs': 300,
        'breed_factor': 0.5,
        'survival_factor': 0.8,
        'top_k_candidates': 3
    }
    if 'portfolio' in req['params']:
        del req['params']['portfolio']
    
    t0 = time.time()
    r = requests.post(URL, json=req, timeout=60)
    dt = time.time() - t0
    
    if r.status_code != 200:
        print(f"seed={seed}: ERROR {r.status_code}")
        continue
    
    data = r.json()
    q = compute_quality(data)
    
    status = "OK" if q['hard_ok'] else "FAIL"
    fail_reason = ""
    if not q['hard_ok']:
        if q['placement_ratio'] < 1.0:
            fail_reason = f" (unplaced={q['unplaced']})"
        elif q['bbox_void'] > 0:
            fail_reason = f" (void={q['bbox_void']:.0f}mm2)"
    
    print(f"seed={seed}: sheets={q['sheets']} waste={q['waste']:.0f} "
          f"perim={q['perim']:.0f} placed={q['placed']}/{q['placed']+q['unplaced']} "
          f"void={q['bbox_void']:.0f} [{status}]{fail_reason} {dt:.1f}s")
