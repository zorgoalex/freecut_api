"""Save top-5 best layouts as SVG files.
Uses guillotine+portfolio, penalty=0.2, 10s, with retry."""
import json, requests, copy, math, os
from collections import deque

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"
OUT_DIR = "ai_docs/tmp/best_layouts_frag02"

os.makedirs(OUT_DIR, exist_ok=True)

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
                    total += grid_mm * grid_mm
    return total

def run_optimize(seed, t_ms=10000):
    req = copy.deepcopy(base_req)
    req['params']['seed'] = seed
    req['params']['layout_mode'] = 'guillotine'
    req['params']['time_limit_ms'] = t_ms
    req['params']['restarts'] = 5
    req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': t_ms}
    
    r = requests.post(URL, json=req, timeout=120)
    if r.status_code != 200: return None
    return r.json()

# Collect all results with retry
all_results = []
print("Collecting layouts from 30 seeds with retry...")

for seed in range(30):
    best = None
    for retry in range(3):
        data = run_optimize(seed + retry * 100)
        if not data: continue
        sheets = data.get('summary', {}).get('used_stock_count', 0)
        total_iv = compute_internal_void(data.get('solutions', []))
        waste = data.get('summary', {}).get('total_waste_area_mm2', 0)
        svg = data.get('artifacts', {}).get('svg', '')
        
        result = {
            'seed': seed + retry * 100, 'sheets': sheets,
            'iv': total_iv, 'waste': waste, 'svg': svg,
            'solutions': data.get('solutions', []),
        }
        
        if total_iv == 0.0:
            best = result
            break
        if best is None or total_iv < best['iv']:
            best = result
    
    if best:
        all_results.append(best)
        status = "OK" if best['iv'] == 0 else f"iv={best['iv']:.0f}"
        print(f"  Seed {seed}: sheets={best['sheets']}, {status}, svg={'yes' if best['svg'] else 'no'}")

# Sort: first by iv (ascending), then by sheets (ascending), then by waste (ascending)
all_results.sort(key=lambda r: (r['iv'], r['sheets'], r['waste']))

print(f"\nTotal results: {len(all_results)}")
print(f"Zero-void results: {sum(1 for r in all_results if r['iv'] == 0)}")

# Save top-5
print(f"\nSaving top-5 to {OUT_DIR}/")
for i, res in enumerate(all_results[:5]):
    fname = f"rank_{i+1:02d}_sheets_{res['sheets']}_iv_{res['iv']:.0f}_seed_{res['seed']}"
    
    # Save SVG
    if res['svg']:
        svg_path = os.path.join(OUT_DIR, fname + ".svg")
        with open(svg_path, 'w', encoding='utf-8') as f:
            f.write(res['svg'])
        print(f"  {fname}.svg ({len(res['svg'])} bytes)")
    else:
        print(f"  {fname} — NO SVG")
    
    # Save JSON with placement details
    json_path = os.path.join(OUT_DIR, fname + ".json")
    json_data = {
        'seed': res['seed'], 'sheets': res['sheets'],
        'internal_void': res['iv'], 'waste': res['waste'],
        'solutions': res['solutions']
    }
    with open(json_path, 'w', encoding='utf-8') as f:
        json.dump(json_data, f, indent=2, ensure_ascii=False)

print(f"\nDone! Files saved to {OUT_DIR}/")
