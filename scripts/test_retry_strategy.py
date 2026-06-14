"""Test retry strategy: if first result has internal void, retry with different seed.
Also test standard mode for comparison."""
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

def run_optimize(seed, layout='guillotine', portfolio=True, t_ms=10000):
    req = copy.deepcopy(base_req)
    req['params']['seed'] = seed
    req['params']['layout_mode'] = layout
    req['params']['time_limit_ms'] = t_ms
    req['params']['restarts'] = 5
    if portfolio:
        req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': t_ms}
    elif 'portfolio' in req['params']:
        del req['params']['portfolio']
    else:
        req['params']['ga_override'] = {'epochs':300,'breed_factor':0.5,'survival_factor':0.8,'top_k_candidates':3}
    
    r = requests.post(URL, json=req, timeout=120)
    if r.status_code != 200: return None
    data = r.json()
    sheets = data.get('summary', {}).get('used_stock_count', 0)
    total_iv = compute_internal_void(data.get('solutions', []))
    placed = sum(len(s.get('placements',[])) or len(s.get('all_pieces',[])) for s in data.get('solutions',[]))
    unplaced = sum(1 for it in data.get('unplaced_items',[]) if it.get('reason')!='oversized')
    pr = placed/(placed+unplaced) if (placed+unplaced)>0 else 1.0
    return {'sheets': sheets, 'iv': total_iv, 'hard_ok': pr == 1.0 and total_iv == 0.0}

# ==========================================
# TEST 1: No retry baseline (20 seeds, portfolio)
# ==========================================
print("=" * 70)
print("TEST 1: guillotine+portfolio (no retry, 20 seeds)")
print("=" * 70)
results = []
for seed in range(20):
    res = run_optimize(seed, portfolio=True)
    if res:
        results.append(res)
        print(f"  Seed {seed:2d}: {'PASS' if res['hard_ok'] else 'FAIL'} sheets={res['sheets']} iv={res['iv']:.0f}")

n_pass = sum(1 for r in results if r['hard_ok'])
n_fail = sum(1 for r in results if not r['hard_ok'])
print(f"\n  RESULT: {n_pass}/{len(results)} pass, {n_fail} fail")

# ==========================================
# TEST 2: With retry (max 3 attempts, portfolio)
# ==========================================
print(f"\n{'=' * 70}")
print("TEST 2: guillotine+portfolio (retry up to 3 attempts)")
print("=" * 70)
retry_results = []
total_attempts = 0
for seed in range(20):
    attempts = 0
    best = None
    for retry in range(3):
        attempts += 1
        total_attempts += 1
        res = run_optimize(seed + retry * 100, portfolio=True)  # different seed each retry
        if res and res['hard_ok']:
            best = res
            break
        if res and (best is None or res['iv'] < best['iv']):
            best = res
    retry_results.append(best)
    status = "PASS" if best and best['hard_ok'] else "FAIL"
    iv_str = f"{best['iv']:.0f}" if best else "0"
    print(f"  Seed {seed:2d}: {status} attempts={attempts} sheets={best['sheets'] if best else '?'} iv={iv_str}")

n_pass_r = sum(1 for r in retry_results if r and r['hard_ok'])
avg_attempts = total_attempts / len(retry_results)
print(f"\n  RESULT: {n_pass_r}/{len(retry_results)} pass, avg attempts={avg_attempts:.1f}")

# ==========================================
# TEST 3: Standard mode baseline (20 seeds, no portfolio)
# ==========================================
print(f"\n{'=' * 70}")
print("TEST 3: guillotine+standard (no retry, 20 seeds, 5s)")
print("=" * 70)
std_results = []
for seed in range(20):
    res = run_optimize(seed, portfolio=False, t_ms=5000)
    if res:
        std_results.append(res)
        print(f"  Seed {seed:2d}: {'PASS' if res['hard_ok'] else 'FAIL'} sheets={res['sheets']} iv={res['iv']:.0f}")

n_pass_s = sum(1 for r in std_results if r['hard_ok'])
n_fail_s = sum(1 for r in std_results if not r['hard_ok'])
avg_sheets_s = sum(r['sheets'] for r in std_results) / len(std_results)
print(f"\n  RESULT: {n_pass_s}/{len(std_results)} pass, {n_fail_s} fail, avg_sheets={avg_sheets_s:.1f}")

# ==========================================
# SUMMARY
# ==========================================
print(f"\n{'=' * 70}")
print("SUMMARY")
print("=" * 70)
print(f"{'Strategy':<35} {'Pass Rate':<12} {'Avg Sheets':<12}")
print("-" * 60)
avg_sheets_p = sum(r['sheets'] for r in results) / len(results) if results else 0
avg_sheets_r = sum(r['sheets'] for r in retry_results if r) / len(retry_results) if retry_results else 0
print(f"{'Portfolio no retry':<35} {n_pass}/{len(results):<12} {avg_sheets_p:<12.1f}")
print(f"{'Portfolio retry (3 attempts)':<35} {n_pass_r}/{len(retry_results):<12} {avg_sheets_r:<12.1f}")
print(f"{'Standard no retry (5s)':<35} {n_pass_s}/{len(std_results):<12} {avg_sheets_s:<12.1f}")
