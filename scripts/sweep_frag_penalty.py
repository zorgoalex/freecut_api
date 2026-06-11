"""Sweep fragmentation penalty values: 0.2, 0.3, 0.4
For each value: edit source, rebuild, test with internal_void metric.
Run this from project root: python scripts/sweep_frag_penalty.py
"""
import json, subprocess, time, sys, os, copy, math, requests
from collections import deque

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(REPO, "tests/fixtures/multisheet_varied_4sheets.json")
URL = "http://localhost:8088/v1/optimize"

PENALTY_VALUES = [0.2, 0.3, 0.4]

MAXRECTS_FILE = os.path.join(REPO, "vendor/cut-optimizer-2d/src/maxrects.rs")
GUILLOTINE_FILE = os.path.join(REPO, "vendor/cut-optimizer-2d/src/guillotine.rs")

with open(FIXTURE) as f:
    base_req = json.load(f)

# --- Internal void computation (flood-fill) ---
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

def set_penalty(value):
    """Set fragmentation penalty in both source files."""
    for fpath in [MAXRECTS_FILE, GUILLOTINE_FILE]:
        with open(fpath, 'r') as f:
            content = f.read()
        # Replace penalty value using regex-like approach
        import re
        new_content = re.sub(
            r'(powf\(2\.0 \+ self\.free_rects\.len\(\) as f64 \* )([\d.]+)(\))',
            lambda m: m.group(1) + str(value) + m.group(3),
            content
        )
        if new_content != content:
            with open(fpath, 'w') as f:
                f.write(new_content)
            print(f"  Updated {os.path.basename(fpath)}: penalty={value}")
        else:
            print(f"  WARNING: No change in {os.path.basename(fpath)}")

def rebuild():
    """Stop service, rebuild, restart."""
    import signal
    print("  Stopping service...")
    try:
        subprocess.run("taskkill /F /IM freecut.exe", shell=True, capture_output=True, timeout=5)
    except: pass
    time.sleep(2)
    
    print("  Building (release)...")
    result = subprocess.run(
        "cargo build --release",
        cwd=REPO, capture_output=True, text=True, shell=True
    )
    if result.returncode != 0:
        print(f"  BUILD FAILED: {result.stderr[-500:]}")
        return False
    print("  Build OK")
    
    print("  Starting service...")
    proc = subprocess.Popen(
        "cargo run --release",
        cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=True
    )
    # Wait for service to be ready
    for i in range(45):
        time.sleep(1)
        try:
            r = requests.get("http://localhost:8088/health/live", timeout=2)
            if r.status_code == 200:
                print(f"  Service ready after {i+1}s")
                return True
        except:
            pass
    print("  Service failed to start!")
    return False

def test_config(name, layout, t_ms, portfolio, ga_ov, seeds=10):
    """Test a single configuration."""
    results = []
    for seed in range(seeds):
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
        return None
    
    hard_oks = sum(1 for r in results if r['hard_ok'])
    avg_sheets = sum(r['sheets'] for r in results) / len(results)
    avg_iv = sum(r['internal_void'] for r in results) / len(results)
    min_iv = min(r['internal_void'] for r in results)
    
    return {
        'hard_ok': hard_oks, 'avg_sheets': avg_sheets,
        'avg_iv': avg_iv, 'min_iv': min_iv, 'n': len(results),
        'details': results
    }

# --- Configs to test ---
configs = [
    ("guill+std 5s", "guillotine", 5000, False, {'epochs':300,'breed_factor':0.5,'survival_factor':0.8,'top_k_candidates':3}),
    ("guill+port 10s", "guillotine", 10000, True, None),
]

# --- Main sweep ---
all_results = {}

for penalty in PENALTY_VALUES:
    print(f"\n{'='*60}")
    print(f"FRAGMENTATION PENALTY = {penalty}")
    print(f"{'='*60}")
    
    set_penalty(penalty)
    if not rebuild():
        print(f"SKIP penalty={penalty} (build failed)")
        continue
    
    all_results[penalty] = {}
    for name, layout, t_ms, portfolio, ga_ov in configs:
        print(f"  Testing {name}...", end=" ", flush=True)
        res = test_config(name, layout, t_ms, portfolio, ga_ov)
        if res:
            all_results[penalty][name] = res
            print(f"sheets={res['avg_sheets']:.1f}, hard_ok={res['hard_ok']}/{res['n']}, avg_iv={res['avg_iv']:.0f}")
        else:
            print("FAILED")

# --- Summary ---
print(f"\n\n{'='*60}")
print("SUMMARY")
print(f"{'='*60}")
print(f"{'Penalty':<10} {'Config':<20} {'Sheets':<8} {'hard_ok':<10} {'avg_iv':<12} {'min_iv':<10}")
print("-" * 70)
for penalty in PENALTY_VALUES:
    if penalty not in all_results: continue
    for name, res in all_results[penalty].items():
        print(f"{penalty:<10} {name:<20} {res['avg_sheets']:<8.1f} {res['hard_ok']}/{res['n']:<8} {res['avg_iv']:<12.0f} {res['min_iv']:<10.0f}")
