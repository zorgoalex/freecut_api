"""Test portfolio candidate_count: 5, 8, 10, 12 with guillotine mode, penalty=0.2."""
import json, requests, copy, math, time
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
    for sol in solutions:
        pl = sol.get('placements', [])
        if not pl: continue
        trim = sol.get('trim_mm') or {}
        uw = sol['width_mm'] - trim.get('left',0) - trim.get('right',0)
        uh = sol['height_mm'] - trim.get('top',0) - trim.get('bottom',0)
        if uw <= 0 or uh <= 0: continue
        grid, rows, cols = build_grid(pl, uw, uh, grid_mm)
        flood_external(grid, rows, cols)
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    total_internal += grid_mm * grid_mm
    return total_internal

CANDIDATE_COUNTS = [5, 8, 10, 12]
TIME_MS = 10000
SEEDS = 10

print("Portfolio candidate_count sweep (guillotine, penalty=0.2, 10s)")
print(f"{'Candidates':<12} {'Sheets':<8} {'hard_ok':<10} {'avg_iv':<12} {'min_iv':<10}")
print("-" * 55)

for cc in CANDIDATE_COUNTS:
    results = []
    t0 = time.time()
    for seed in range(SEEDS):
        req = copy.deepcopy(base_req)
        req['params']['seed'] = seed
        req['params']['layout_mode'] = 'guillotine'
        req['params']['time_limit_ms'] = TIME_MS
        req['params']['restarts'] = 5
        req['params']['portfolio'] = {'enabled': True, 'candidate_count': cc, 'deadline_ms': TIME_MS}
        
        try:
            r = requests.post(URL, json=req, timeout=120)
            if r.status_code != 200: continue
            data = r.json()
        except:
            continue
        
        sheets = data.get('summary', {}).get('used_stock_count', 0)
        total_iv = compute_internal_void(data.get('solutions', []))
        placed = sum(len(s.get('placements',[])) for s in data.get('solutions',[]))
        unplaced = sum(1 for it in data.get('unplaced_items',[]) if it.get('reason')!='oversized')
        pr = placed/(placed+unplaced) if (placed+unplaced)>0 else 1.0
        hard_ok = pr == 1.0 and total_iv == 0.0
        results.append({'sheets': sheets, 'internal_void': total_iv, 'hard_ok': hard_ok})
    
    if not results:
        print(f"{cc:<12} NO RESULTS")
        continue
    
    elapsed = time.time() - t0
    hard_oks = sum(1 for r in results if r['hard_ok'])
    avg_sheets = sum(r['sheets'] for r in results) / len(results)
    avg_iv = sum(r['internal_void'] for r in results) / len(results)
    min_iv = min(r['internal_void'] for r in results)
    
    print(f"{cc:<12} {avg_sheets:<8.1f} {hard_oks}/{len(results):<8} {avg_iv:<12.0f} {min_iv:<10.0f}  ({elapsed:.1f}s)")
