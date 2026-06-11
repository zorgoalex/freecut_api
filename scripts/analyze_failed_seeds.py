"""Diagnose failed seeds: compare passing vs failing seeds in detail.
Run with guillotine+portfolio, penalty=0.2, 10s."""
import json, requests, copy, math
from collections import deque

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"

with open(FIXTURE) as f:
    base_req = json.load(f)

def build_grid(placements, uw, uh, grid_mm):
    cols = int(math.ceil(uw / grid_mm))
    rows = int(math.ceil(uh / grid_mm))
    grid = [[0]*cols for _ in range(rows)]
    for p in placements:
        x0 = max(0, int(p['x_mm']/grid_mm))
        y0 = max(0, int(p['y_mm']/grid_mm))
        x1 = min(cols, int(math.ceil((p['x_mm']+p['width_mm'])/grid_mm)))
        y1 = min(rows, int(math.ceil((p['y_mm']+p['height_mm'])/grid_mm)))
        for r in range(y0, y1):
            for c in range(x0, x1):
                grid[r][c] = 1
    return grid, rows, cols

def flood_external(grid, rows, cols):
    q = deque()
    def enq(r,c):
        if 0<=r<rows and 0<=c<cols and grid[r][c]==0:
            grid[r][c]=2; q.append((r,c))
    for c in range(cols):
        enq(0,c)
        if rows>1: enq(rows-1,c)
    for r in range(rows):
        enq(r,0)
        if cols>1: enq(r,cols-1)
    while q:
        r,c = q.popleft()
        for nr,nc in [(r-1,c),(r+1,c),(r,c-1),(r,c+1)]:
            if 0<=nr<rows and 0<=nc<cols and grid[nr][nc]==0:
                grid[nr][nc]=2; q.append((nr,nc))

def compute_internal_void(solutions, grid_mm=5.0):
    total = 0.0
    per_sheet = []
    for sol in solutions:
        pl = sol.get('placements', [])
        if not pl: continue
        trim = sol.get('trim_mm') or {}
        uw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
        uh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
        if uw <= 0 or uh <= 0: continue
        grid, rows, cols = build_grid(pl, uw, uh, grid_mm)
        flood_external(grid, rows, cols)
        iv = 0.0
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    iv += grid_mm * grid_mm
        total += iv
        per_sheet.append(iv)
    return total, per_sheet

def analyze_sheet(solutions):
    """Detailed per-sheet analysis."""
    info = []
    for i, sol in enumerate(solutions):
        pl = sol.get('placements', [])
        all_p = sol.get('all_pieces', pl)
        pieces = all_p if all_p else pl
        n_pieces = len(pieces)
        
        # Piece sizes
        areas = [p.get('width_mm',0) * p.get('height_mm',0) for p in pieces]
        total_area = sum(areas)
        
        trim = sol.get('trim_mm') or {}
        uw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
        uh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
        sheet_area = uw * uh
        util = total_area / sheet_area if sheet_area > 0 else 0
        
        # Find piece position patterns
        xs = [p.get('x_mm', 0) for p in pieces]
        ys = [p.get('y_mm', 0) for p in pieces]
        ws = [p.get('width_mm', 0) for p in pieces]
        hs = [p.get('height_mm', 0) for p in pieces]
        
        # Small pieces (area < 50000 mm2 = ~224x224)
        small = [(p['width_mm'], p['height_mm'], p.get('x_mm',0), p.get('y_mm',0)) 
                 for p in pieces if p.get('width_mm',0)*p.get('height_mm',0) < 50000]
        
        info.append({
            'sheet': i, 'n_pieces': n_pieces, 'utilization': util,
            'total_area': total_area, 'sheet_area': sheet_area,
            'small_pieces': len(small),
            'small_details': small[:5],  # first 5 small pieces
            'x_range': (min(xs) if xs else 0, max(x+w for x,w in zip(xs,ws)) if xs else 0),
            'y_range': (min(ys) if ys else 0, max(y+h for y,h in zip(ys,hs)) if ys else 0),
        })
    return info

# Run for 20 seeds to get enough failed cases
print("Running 20 seeds (guillotine+portfolio, penalty=0.2, 10s)...")
print("="*80)

passes = []
fails = []

for seed in range(20):
    req = copy.deepcopy(base_req)
    req['params']['seed'] = seed
    req['params']['layout_mode'] = 'guillotine'
    req['params']['time_limit_ms'] = 10000
    req['params']['restarts'] = 5
    req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': 10000}
    
    try:
        r = requests.post(URL, json=req, timeout=120)
        if r.status_code != 200:
            print(f"Seed {seed}: HTTP {r.status_code}")
            continue
        data = r.json()
    except Exception as e:
        print(f"Seed {seed}: ERROR {e}")
        continue
    
    sheets = data.get('summary', {}).get('used_stock_count', 0)
    total_iv, per_sheet_iv = compute_internal_void(data.get('solutions', []))
    placed = sum(len(s.get('placements',[])) or len(s.get('all_pieces',[])) for s in data.get('solutions',[]))
    unplaced = sum(1 for it in data.get('unplaced_items',[]) if it.get('reason')!='oversized')
    pr = placed/(placed+unplaced) if (placed+unplaced)>0 else 1.0
    hard_ok = pr == 1.0 and total_iv == 0.0
    
    sheet_info = analyze_sheet(data.get('solutions', []))
    
    status = "PASS" if hard_ok else "FAIL"
    result = {
        'seed': seed, 'sheets': sheets, 'hard_ok': hard_ok,
        'total_iv': total_iv, 'per_sheet_iv': per_sheet_iv,
        'placed': placed, 'unplaced': unplaced,
        'sheet_info': sheet_info
    }
    
    if hard_ok:
        passes.append(result)
    else:
        fails.append(result)
    
    print(f"Seed {seed:2d}: {status} sheets={sheets} iv={total_iv:.0f} placed={placed} unplaced={unplaced}")

print(f"\n{'='*80}")
print(f"SUMMARY: {len(passes)} pass, {len(fails)} fail out of {len(passes)+len(fails)} seeds")

if fails:
    print(f"\n{'='*80}")
    print("FAILED SEEDS DETAIL:")
    print(f"{'='*80}")
    for f in fails:
        print(f"\nSeed {f['seed']}: sheets={f['sheets']}, iv={f['total_iv']:.0f}")
        print(f"  Per-sheet IV: {[f'{v:.0f}' for v in f['per_sheet_iv']]}")
        for si in f['sheet_info']:
            print(f"  Sheet {si['sheet']}: {si['n_pieces']} pcs, util={si['utilization']:.1%}, "
                  f"small={si['small_pieces']}, "
                  f"x=[{si['x_range'][0]:.0f}-{si['x_range'][1]:.0f}], "
                  f"y=[{si['y_range'][0]:.0f}-{si['y_range'][1]:.0f}]")
            if si['small_details']:
                for sd in si['small_details']:
                    print(f"    small: {sd[0]:.0f}x{sd[1]:.0f} @ ({sd[2]:.0f},{sd[3]:.0f})")

if passes:
    print(f"\n{'='*80}")
    print("PASSING SEEDS DETAIL (first 3):")
    print(f"{'='*80}")
    for p in passes[:3]:
        print(f"\nSeed {p['seed']}: sheets={p['sheets']}, iv={p['total_iv']:.0f}")
        for si in p['sheet_info']:
            print(f"  Sheet {si['sheet']}: {si['n_pieces']} pcs, util={si['utilization']:.1%}, "
                  f"small={si['small_pieces']}")

# Compare stats
if passes and fails:
    print(f"\n{'='*80}")
    print("PASS vs FAIL COMPARISON:")
    print(f"{'='*80}")
    avg_pass_sheets = sum(p['sheets'] for p in passes) / len(passes)
    avg_fail_sheets = sum(f['sheets'] for f in fails) / len(fails)
    avg_pass_util = sum(si['utilization'] for p in passes for si in p['sheet_info']) / sum(len(p['sheet_info']) for p in passes)
    avg_fail_util = sum(si['utilization'] for f in fails for si in f['sheet_info']) / sum(len(f['sheet_info']) for f in fails)
    avg_pass_pieces = sum(si['n_pieces'] for p in passes for si in p['sheet_info']) / sum(len(p['sheet_info']) for p in passes)
    avg_fail_pieces = sum(si['n_pieces'] for f in fails for si in f['sheet_info']) / sum(len(f['sheet_info']) for f in fails)
    avg_pass_small = sum(si['small_pieces'] for p in passes for si in p['sheet_info']) / sum(len(p['sheet_info']) for p in passes)
    avg_fail_small = sum(si['small_pieces'] for f in fails for si in f['sheet_info']) / sum(len(f['sheet_info']) for f in fails)
    
    print(f"  Sheets:       pass={avg_pass_sheets:.1f}  fail={avg_fail_sheets:.1f}")
    print(f"  Utilization:  pass={avg_pass_util:.1%}  fail={avg_fail_util:.1%}")
    print(f"  Pieces/sheet: pass={avg_pass_pieces:.1f}  fail={avg_fail_pieces:.1f}")
    print(f"  Small pieces: pass={avg_pass_small:.1f}  fail={avg_fail_small:.1f}")
