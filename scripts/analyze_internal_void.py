"""Compute proper internal_void using flood-fill like deep_param_sweep.py."""
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
    total_internal = 0.0
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
        sheet_internal = 0.0
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    sheet_internal += grid_mm * grid_mm
        total_internal += sheet_internal
        per_sheet.append(sheet_internal)
    return total_internal, per_sheet

configs = [
    ("nested+portfolio 5s", "nested", 5000, True, None),
    ("nested+portfolio 10s", "nested", 10000, True, None),
    ("nested+standard 5s", "nested", 5000, False, {'epochs':300,'breed_factor':0.5,'survival_factor':0.8,'top_k_candidates':3}),
    ("guillotine+portfolio 10s", "guillotine", 10000, True, None),
    ("guillotine+standard 5s", "guillotine", 5000, False, {'epochs':300,'breed_factor':0.5,'survival_factor':0.8,'top_k_candidates':3}),
]

for name, layout, t_ms, portfolio, ga_ov in configs:
    results = []
    for seed in range(10):
        req = copy.deepcopy(base_req)
        req['params']['seed'] = seed
        req['params']['layout_mode'] = layout
        req['params']['time_limit_ms'] = t_ms
        req['params']['restarts'] = 5
        if portfolio:
            req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': t_ms}
        elif 'portfolio' in req['params']:
            del req['params']['portfolio']
        if ga_ov:
            req['params']['ga_override'] = ga_ov
        
        r = requests.post(URL, json=req, timeout=120)
        if r.status_code != 200: continue
        data = r.json()
        
        cs = data.get('summary', {}).get('candidate_selection', {})
        sheets = data.get('summary', {}).get('used_stock_count', 0)
        bbox_void = cs.get('winner_bbox_void_area_mm2', 0)
        
        total_iv, per_sheet = compute_internal_void(data.get('solutions', []))
        placed = sum(len(s.get('placements',[])) for s in data.get('solutions',[]))
        unplaced = sum(1 for it in data.get('unplaced_items',[]) if it.get('reason')!='oversized')
        pr = placed/(placed+unplaced) if (placed+unplaced)>0 else 1.0
        hard_ok = pr == 1.0 and total_iv == 0.0
        
        results.append({'sheets': sheets, 'bbox_void': bbox_void, 'internal_void': total_iv,
                        'hard_ok': hard_ok, 'per_sheet_iv': per_sheet})
    
    if not results:
        print(f"{name}: NO RESULTS")
        continue
    
    hard_oks = sum(1 for r in results if r['hard_ok'])
    avg_sheets = sum(r['sheets'] for r in results) / len(results)
    avg_iv = sum(r['internal_void'] for r in results) / len(results)
    avg_bv = sum(r['bbox_void'] for r in results) / len(results)
    min_iv = min(r['internal_void'] for r in results)
    
    print(f"\n{name}:")
    print(f"  sheets={avg_sheets:.1f}, hard_ok={hard_oks}/10")
    print(f"  avg_internal_void={avg_iv:.0f} mm2 (min={min_iv:.0f})")
    print(f"  avg_bbox_void={avg_bv:.0f} mm2")
    # Show per-sheet breakdown for best result
    best = min(results, key=lambda r: r['internal_void'])
    print(f"  Best: sheets={best['sheets']}, iv={best['internal_void']:.0f}, per_sheet={[f'{v:.0f}' for v in best['per_sheet_iv']]}")
