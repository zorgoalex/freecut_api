import json
import random
import requests
import time
import os
import math
import csv
from collections import deque
import subprocess

def _env_float(name, default):
    raw = os.getenv(name)
    return float(raw) if raw is not None else default


def _env_int(name, default):
    raw = os.getenv(name)
    return int(raw) if raw is not None else default


def _env_bool(name, default):
    raw = os.getenv(name)
    if raw is None:
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


# Configuration (overridable via environment variables)
API_URL = os.getenv("FREECUT_API_URL", "http://localhost:8088/v1/optimize")
INPUT_FILE = os.getenv("FREECUT_INPUT_FILE", "ai_docs/test_11items_optimize_request.json")
CANDIDATES_DIR = os.getenv("FREECUT_CANDIDATES_DIR", "ai_docs/candidate_layouts")
LOG_FILE = os.getenv("FREECUT_LOG_FILE", "ai_docs/optimization_log.csv")
NUM_TESTS = _env_int("FREECUT_NUM_TESTS", 500)
TOP_N_CANDIDATES = _env_int("FREECUT_TOP_N", 10)
GRID_MM = _env_float("FREECUT_GRID_MM", 5.0)
USE_PADDED_GAPS = _env_bool("FREECUT_USE_PADDED_GAPS", True)
CURLIMAGE = os.getenv("FREECUT_CURLIMAGE", "curlimages/curl:8.6.0")
USE_CURLIMAGE = _env_bool("FREECUT_USE_CURLIMAGE", False)
EDGE_PENALTY_POW = _env_float("FREECUT_EDGE_PENALTY_POW", 1.0)
CORRIDOR_OK_MULT = _env_float("FREECUT_CORRIDOR_OK_MULT", 3.0)

def load_request_template(filepath):
    with open(filepath, 'r') as f:
        return json.load(f)

def calculate_bbox_metrics(placements):
    if not placements:
        return None

    min_x = float('inf')
    min_y = float('inf')
    max_x = float('-inf')
    max_y = float('-inf')
    total_parts_area = 0.0

    for placement in placements:
        x = placement['x_mm']
        y = placement['y_mm']
        w = placement['width_mm']
        h = placement['height_mm']

        min_x = min(min_x, x)
        min_y = min(min_y, y)
        max_x = max(max_x, x + w)
        max_y = max(max_y, y + h)
        total_parts_area += w * h

    return {
        'min_x': min_x,
        'min_y': min_y,
        'max_x': max_x,
        'max_y': max_y,
        'total_parts_area': total_parts_area
    }


def calculate_compactness_score(solutions):
    """
    Calculates a compactness score based on the 'internal gap area'.
    This metric finds the bounding box of all placed items and subtracts the
    total area of the items from the bounding box area.
    A lower score indicates a more compact cluster of parts with fewer internal gaps.
    """
    if not solutions:
        return float('inf')

    total_bbox_void = 0.0
    for solution in solutions:
        placements = solution.get('placements', [])
        metrics = calculate_bbox_metrics(placements)
        if not metrics:
            continue

        bounding_box_width = metrics['max_x'] - metrics['min_x']
        bounding_box_height = metrics['max_y'] - metrics['min_y']
        bounding_box_area = bounding_box_width * bounding_box_height
        total_bbox_void += bounding_box_area - metrics['total_parts_area']

    return total_bbox_void


def build_occupancy_grid(placements, usable_w, usable_h, grid_mm, pad_mm):
    cols = max(1, int(math.ceil(usable_w / grid_mm)))
    rows = max(1, int(math.ceil(usable_h / grid_mm)))
    grid = [bytearray(cols) for _ in range(rows)]

    for placement in placements:
        x0 = placement['x_mm'] - pad_mm
        y0 = placement['y_mm'] - pad_mm
        x1 = placement['x_mm'] + placement['width_mm'] + pad_mm
        y1 = placement['y_mm'] + placement['height_mm'] + pad_mm

        if x1 <= 0 or y1 <= 0 or x0 >= usable_w or y0 >= usable_h:
            continue

        gx0 = max(0, int(math.floor(x0 / grid_mm)))
        gy0 = max(0, int(math.floor(y0 / grid_mm)))
        gx1 = min(cols - 1, int(math.ceil(x1 / grid_mm)) - 1)
        gy1 = min(rows - 1, int(math.ceil(y1 / grid_mm)) - 1)

        if gx1 < gx0 or gy1 < gy0:
            continue

        fill = b'\x01' * (gx1 - gx0 + 1)
        for gy in range(gy0, gy1 + 1):
            row = grid[gy]
            row[gx0:gx1 + 1] = fill

    return grid, rows, cols


def flood_external_voids(grid, rows, cols):
    q = deque()

    def enqueue(r, c):
        if grid[r][c] == 0:
            grid[r][c] = 2
            q.append((r, c))

    for c in range(cols):
        enqueue(0, c)
        if rows > 1:
            enqueue(rows - 1, c)
    for r in range(rows):
        enqueue(r, 0)
        if cols > 1:
            enqueue(r, cols - 1)

    while q:
        r, c = q.popleft()
        if r > 0:
            enqueue(r - 1, c)
        if r + 1 < rows:
            enqueue(r + 1, c)
        if c > 0:
            enqueue(r, c - 1)
        if c + 1 < cols:
            enqueue(r, c + 1)


def compute_span_lengths(grid, rows, cols, value):
    row_span = [[0] * cols for _ in range(rows)]
    col_span = [[0] * cols for _ in range(rows)]

    for r in range(rows):
        c = 0
        while c < cols:
            if grid[r][c] != value:
                c += 1
                continue
            start = c
            while c < cols and grid[r][c] == value:
                c += 1
            length = c - start
            for k in range(start, c):
                row_span[r][k] = length

    for c in range(cols):
        r = 0
        while r < rows:
            if grid[r][c] != value:
                r += 1
                continue
            start = r
            while r < rows and grid[r][c] == value:
                r += 1
            length = r - start
            for k in range(start, r):
                col_span[k][c] = length

    return row_span, col_span


def distance_to_edge_mm(r, c, rows, cols, grid_mm):
    return min(r, c, rows - 1 - r, cols - 1 - c) * grid_mm


def calculate_internal_void_metrics(solutions, grid_mm, pad_mm=0.0, spacing_mm=0.0, corridor_ok_mult=3.0):
    if not solutions:
        return float('inf'), float('inf')

    total_internal_area = 0.0
    total_components = 0
    total_exposure_penalty = 0.0
    total_corridor_area = 0.0
    total_corridor_weighted_area = 0.0
    total_corridor_components = 0
    total_row_gap_area = 0.0
    total_col_gap_area = 0.0
    corridor_ok_mm = max(0.0, spacing_mm * corridor_ok_mult)

    for solution in solutions:
        placements = solution.get('placements', [])
        if not placements:
            continue

        trim = solution.get('trim_mm') or {}
        usable_w = solution['width_mm'] - trim.get('left', 0.0) - trim.get('right', 0.0)
        usable_h = solution['height_mm'] - trim.get('top', 0.0) - trim.get('bottom', 0.0)
        if usable_w <= 0 or usable_h <= 0:
            continue

        grid, rows, cols = build_occupancy_grid(
            placements,
            usable_w,
            usable_h,
            grid_mm,
            pad_mm
        )

        flood_external_voids(grid, rows, cols)
        row_span, col_span = compute_span_lengths(grid, rows, cols, 2)

        bbox = calculate_bbox_metrics(placements)
        if not bbox:
            continue

        bx0 = max(0, int(math.floor(bbox['min_x'] / grid_mm)))
        by0 = max(0, int(math.floor(bbox['min_y'] / grid_mm)))
        bx1 = min(cols - 1, int(math.ceil(bbox['max_x'] / grid_mm)) - 1)
        by1 = min(rows - 1, int(math.ceil(bbox['max_y'] / grid_mm)) - 1)

        # Penalize exposed part edges that face empty space far from the sheet edge.
        def corridor_width_mm(r, c):
            if grid[r][c] != 2:
                return 0.0
            return min(row_span[r][c], col_span[r][c]) * grid_mm

        def is_wide_void(r, c):
            if grid[r][c] != 2:
                return False
            if corridor_ok_mm <= 0.0:
                return True
            return corridor_width_mm(r, c) > corridor_ok_mm

        def width_factor(r, c):
            if corridor_ok_mm <= 0.0:
                return 1.0
            width_mm = corridor_width_mm(r, c)
            if width_mm <= corridor_ok_mm:
                return 1.0
            return width_mm / corridor_ok_mm

        for r in range(rows):
            for c in range(cols):
                if grid[r][c] != 1:
                    continue
                # Neighbor checks
                if r > 0 and is_wide_void(r - 1, c):
                    factor = width_factor(r - 1, c)
                    d = distance_to_edge_mm(r - 1, c, rows, cols, grid_mm)
                    total_exposure_penalty += (d ** EDGE_PENALTY_POW) * grid_mm * factor
                if r + 1 < rows and is_wide_void(r + 1, c):
                    factor = width_factor(r + 1, c)
                    d = distance_to_edge_mm(r + 1, c, rows, cols, grid_mm)
                    total_exposure_penalty += (d ** EDGE_PENALTY_POW) * grid_mm * factor
                if c > 0 and is_wide_void(r, c - 1):
                    factor = width_factor(r, c - 1)
                    d = distance_to_edge_mm(r, c - 1, rows, cols, grid_mm)
                    total_exposure_penalty += (d ** EDGE_PENALTY_POW) * grid_mm * factor
                if c + 1 < cols and is_wide_void(r, c + 1):
                    factor = width_factor(r, c + 1)
                    d = distance_to_edge_mm(r, c + 1, rows, cols, grid_mm)
                    total_exposure_penalty += (d ** EDGE_PENALTY_POW) * grid_mm * factor

        internal_cells = 0
        for r in range(rows):
            for c in range(cols):
                if grid[r][c] == 0:
                    total_components += 1
                    q = deque([(r, c)])
                    grid[r][c] = 3
                    while q:
                        cr, cc = q.popleft()
                        internal_cells += 1
                        if cr > 0 and grid[cr - 1][cc] == 0:
                            grid[cr - 1][cc] = 3
                            q.append((cr - 1, cc))
                        if cr + 1 < rows and grid[cr + 1][cc] == 0:
                            grid[cr + 1][cc] = 3
                            q.append((cr + 1, cc))
                        if cc > 0 and grid[cr][cc - 1] == 0:
                            grid[cr][cc - 1] = 3
                            q.append((cr, cc - 1))
                        if cc + 1 < cols and grid[cr][cc + 1] == 0:
                            grid[cr][cc + 1] = 3
                            q.append((cr, cc + 1))

        total_internal_area += internal_cells * (grid_mm * grid_mm)

        corridor_cells = 0
        for r in range(by0, by1 + 1):
            for c in range(bx0, bx1 + 1):
                if is_wide_void(r, c):
                    total_corridor_components += 1
                    q = deque([(r, c)])
                    grid[r][c] = 4
                    while q:
                        cr, cc = q.popleft()
                        corridor_cells += 1
                        factor = width_factor(cr, cc)
                        total_corridor_weighted_area += (grid_mm * grid_mm) * (factor * factor)
                        if cr > by0 and is_wide_void(cr - 1, cc):
                            grid[cr - 1][cc] = 4
                            q.append((cr - 1, cc))
                        if cr < by1 and is_wide_void(cr + 1, cc):
                            grid[cr + 1][cc] = 4
                            q.append((cr + 1, cc))
                        if cc > bx0 and is_wide_void(cr, cc - 1):
                            grid[cr][cc - 1] = 4
                            q.append((cr, cc - 1))
                        if cc < bx1 and is_wide_void(cr, cc + 1):
                            grid[cr][cc + 1] = 4
                            q.append((cr, cc + 1))

        total_corridor_area += corridor_cells * (grid_mm * grid_mm)

        row_gap_cells = 0
        for r in range(rows):
            min_c = None
            max_c = None
            for c in range(cols):
                if grid[r][c] == 1:
                    if min_c is None or c < min_c:
                        min_c = c
                    if max_c is None or c > max_c:
                        max_c = c
            if min_c is None or max_c is None or max_c <= min_c:
                continue
            for c in range(min_c, max_c + 1):
                if grid[r][c] != 1:
                    row_gap_cells += 1

        col_gap_cells = 0
        for c in range(cols):
            min_r = None
            max_r = None
            for r in range(rows):
                if grid[r][c] == 1:
                    if min_r is None or r < min_r:
                        min_r = r
                    if max_r is None or r > max_r:
                        max_r = r
            if min_r is None or max_r is None or max_r <= min_r:
                continue
            for r in range(min_r, max_r + 1):
                if grid[r][c] != 1:
                    col_gap_cells += 1

        total_row_gap_area += row_gap_cells * (grid_mm * grid_mm)
        total_col_gap_area += col_gap_cells * (grid_mm * grid_mm)

    if total_internal_area == 0.0:
        total_internal_area = 0.0

    return (
        total_internal_area,
        total_components,
        total_exposure_penalty,
        total_corridor_area,
        total_corridor_components,
        total_row_gap_area,
        total_col_gap_area,
        total_corridor_weighted_area,
    )


def run_optimization(template, restarts, time_limit, seed):
    payload = template.copy()
    
    # --- KEY CHANGE: GUILLOTINE MODE ---
    payload['params']['layout_mode'] = 'guillotine'
    
    payload['params']['restarts'] = restarts
    payload['params']['time_limit_ms'] = time_limit
    payload['params']['seed'] = seed
    
    timeout_s = (time_limit / 1000.0) + 5
    if USE_CURLIMAGE:
        try:
            payload_str = json.dumps(payload)
            cmd = [
                "docker",
                "run",
                "--rm",
                "--network",
                "host",
                CURLIMAGE,
                "-s",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                payload_str,
                API_URL,
            ]
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout_s + 5,
                check=False,
            )
            if result.returncode != 0 or not result.stdout:
                return None
            return json.loads(result.stdout)
        except (subprocess.SubprocessError, json.JSONDecodeError):
            return None

    try:
        response = requests.post(API_URL, json=payload, timeout=timeout_s)
        if response.status_code == 200:
            return response.json()
        return None
    except requests.exceptions.RequestException:
        return None

def main():
    if not os.path.exists(INPUT_FILE):
        print(f"Error: Input file {INPUT_FILE} not found.")
        return

    # --- Setup output directories and log file ---
    os.makedirs(CANDIDATES_DIR, exist_ok=True)
    
    print(f"Loading template from {INPUT_FILE}...")
    template = load_request_template(INPUT_FILE)
    spacing_mm = float(template['params'].get('spacing_mm', 0.0))
    corridor_ok_mm = spacing_mm * CORRIDOR_OK_MULT
    # stock_width and stock_height are not needed for the new compactness score, 
    # but could be useful for other metrics. Keeping them for context for now.
    # stock_width = template['stock'][0]['width_mm']
    # stock_height = template['stock'][0]['height_mm']
    
    top_candidates = []
    
    log_header = [
        'restarts',
        'time_limit_ms',
        'seed',
        'waste_percent',
        'compactness_score',
        'internal_void_mm2',
        'internal_components',
        'corridor_void_mm2',
        'corridor_weighted_mm2',
        'corridor_components',
        'row_gap_mm2',
        'col_gap_mm2',
        'edge_gap_sum_mm',
        'exposure_penalty',
        'grid_mm',
        'pad_mm',
        'spacing_mm',
        'corridor_ok_mm'
    ]
    with open(LOG_FILE, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow(log_header)

        print(f"Starting {NUM_TESTS} optimization tests (Mode: GUILLOTINE)...")
        print(f"Target: Find Top {TOP_N_CANDIDATES} candidates by Waste %, internal voids, and corridor-aware gaps.")
        print(f"Logging all runs to {LOG_FILE}")
        print(f"{'Iter':<5} | {'Restarts':<8} | {'Time':<5} | {'Waste%':<7} | {'IntVoid':<9} | {'RowGap':<9} | {'ColGap':<9} | {'Status'}")
        print("-" * 80)

        for i in range(1, NUM_TESTS + 1):
            restarts = random.randint(50, 800)
            time_limit = random.randint(2000, 12000)
            seed = random.randint(1, 10000000)
            
            result = run_optimization(template, restarts, time_limit, seed)
            
            status_str = "FAILED"
            if result and result.get('status') == 'ok':
                summary = result['summary']
                waste = summary['waste_percent']
                
                # Use the new compactness score
                bbox_void = calculate_compactness_score(result['solutions'])
                pad_mm = 0.0
                if USE_PADDED_GAPS:
                    pad_mm = (template['params']['kerf_mm'] + template['params']['spacing_mm']) / 2.0
                internal_void, internal_components, exposure_penalty, corridor_void, corridor_components, row_gap_area, col_gap_area, corridor_weighted_area = calculate_internal_void_metrics(
                    result['solutions'],
                    GRID_MM,
                    pad_mm,
                    spacing_mm=spacing_mm,
                    corridor_ok_mult=CORRIDOR_OK_MULT
                )
                strip_gap_area = row_gap_area + col_gap_area
                edge_gap_sum = 0.0
                for solution in result['solutions']:
                    placements = solution.get('placements', [])
                    metrics = calculate_bbox_metrics(placements)
                    if not metrics:
                        continue
                    trim = solution.get('trim_mm') or {}
                    usable_w = solution['width_mm'] - trim.get('left', 0.0) - trim.get('right', 0.0)
                    usable_h = solution['height_mm'] - trim.get('top', 0.0) - trim.get('bottom', 0.0)
                    edge_gap_sum += (
                        metrics['min_x'] +
                        metrics['min_y'] +
                        max(0.0, usable_w - metrics['max_x']) +
                        max(0.0, usable_h - metrics['max_y'])
                    )
                
                # Log every run
                writer.writerow([
                    restarts,
                    time_limit,
                    seed,
                    waste,
                    bbox_void,
                    internal_void,
                    internal_components,
                    corridor_void,
                    corridor_weighted_area,
                    corridor_components,
                    row_gap_area,
                    col_gap_area,
                    edge_gap_sum,
                    exposure_penalty,
                    GRID_MM,
                    pad_mm,
                    spacing_mm,
                    corridor_ok_mm
                ])
                
                # --- Candidate Management ---
                candidate_data = {
                    'restarts': restarts,
                    'time_limit_ms': time_limit,
                    'seed': seed,
                    'waste_percent': waste,
                    'bbox_void': bbox_void,
                    'internal_void': internal_void,
                    'internal_components': internal_components,
                    'corridor_void': corridor_void,
                    'corridor_weighted_area': corridor_weighted_area,
                    'corridor_components': corridor_components,
                    'row_gap_area': row_gap_area,
                    'col_gap_area': col_gap_area,
                    'strip_gap_area': strip_gap_area,
                    'edge_gap_sum': edge_gap_sum,
                    'exposure_penalty': exposure_penalty,
                    'svg': result['artifacts']['svg']
                }

                # Add to list and sort
                top_candidates.append(candidate_data)
                # Sort by waste, internal voids, corridor-weighted voids, then gaps.
                top_candidates.sort(key=lambda c: (
                    c['waste_percent'],
                    c['internal_void'],
                    c['corridor_weighted_area'],
                    c['corridor_void'],
                    c['exposure_penalty'],
                    c['strip_gap_area'],
                    c['corridor_components'],
                    c['internal_components'],
                    c['edge_gap_sum'],
                    c['bbox_void']
                ))
                
                # Trim the list if it's too long
                if len(top_candidates) > TOP_N_CANDIDATES:
                    top_candidates.pop() # Removes the worst candidate

                status_str = f"OK (Waste: {waste:.2f}%)"
                print(f"{i:<5} | {restarts:<8} | {time_limit:<5} | {waste:<7.2f} | {internal_void:<9.0f} | {row_gap_area:<9.0f} | {col_gap_area:<9.0f} | {status_str}")
            else:
                 print(f"{i:<5} | {restarts:<8} | {time_limit:<5} | {'-':<7} | {'-':<9} | {'-':<9} | {'-':<9} | {status_str}")


    print("\n" + "="*40)
    print("Optimization Search Completed.")
    print("="*40)

    if not top_candidates:
        print("No successful optimization runs found.")
        return

    print(f"\nTop {len(top_candidates)} Candidates (Sorted by Waste, then corridor-aware score):")
    print("-" * 80)
    print(f"{'Rank':<5} | {'Waste%':<7} | {'IntVoid':<9} | {'RowGap':<9} | {'ColGap':<9} | {'CorrVoid':<9} | {'Restarts':<8} | {'Time':<5} | {'Seed'}")
    print("-" * 80)

    for idx, candidate in enumerate(top_candidates):
        rank = idx + 1
        # Save SVG to file
        svg_filename = (
            f"rank_{rank:02d}_waste_{candidate['waste_percent']:.2f}"
            f"_void_{candidate['internal_void']:.0f}"
            f"_gaps_{candidate['internal_components']}"
            f".svg"
        )
        svg_filepath = os.path.join(CANDIDATES_DIR, svg_filename)
        with open(svg_filepath, 'w') as f:
            f.write(candidate['svg'])

        print(
            f"{rank:<5} | {candidate['waste_percent']:<7.2f} | "
            f"{candidate['internal_void']:<9.0f} | {candidate['row_gap_area']:<9.0f} | "
            f"{candidate['col_gap_area']:<9.0f} | {candidate['corridor_void']:<9.0f} | "
            f"{candidate['restarts']:<8} | {candidate['time_limit_ms']:<5} | {candidate['seed']}"
        )

    print("-" * 80)
    print(f"\nSaved {len(top_candidates)} candidate SVGs to: {CANDIDATES_DIR}")
    print(f"Full run data logged to: {LOG_FILE}")
    print("\nAnalysis of the relationship between parameters can now be performed on this log file.")

if __name__ == "__main__":
    main()
