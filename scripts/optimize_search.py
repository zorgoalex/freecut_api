import json
import random
import requests
import time
import os
import math
import csv

# Configuration
API_URL = "http://localhost:8088/v1/optimize"
INPUT_FILE = "ai_docs/test_11items_optimize_request.json"
CANDIDATES_DIR = "ai_docs/candidate_layouts"
LOG_FILE = "ai_docs/optimization_log.csv"
NUM_TESTS = 500  # Increased for better analysis
TOP_N_CANDIDATES = 10

def load_request_template(filepath):
    with open(filepath, 'r') as f:
        return json.load(f)

def calculate_compactness_score(solutions):
    """
    Calculates a compactness score based on the 'internal gap area'.
    This metric finds the bounding box of all placed items and subtracts the
    total area of the items from the bounding box area.
    A lower score indicates a more compact cluster of parts with fewer internal gaps.
    """
    if not solutions:
        return float('inf')
    
    solution = solutions[0] # Assuming single sheet
    placements = solution.get('placements', [])
    
    if not placements:
        return float('inf')

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
    
    # Calculate bounding box area of the cluster of parts
    bounding_box_width = max_x - min_x
    bounding_box_height = max_y - min_y
    bounding_box_area = bounding_box_width * bounding_box_height

    # Internal gap area = Bounding Box Area - Total Parts Area
    # This score is minimized
    return bounding_box_area - total_parts_area


def run_optimization(template, restarts, time_limit, seed):
    payload = template.copy()
    
    # --- KEY CHANGE: GUILLOTINE MODE ---
    payload['params']['layout_mode'] = 'guillotine'
    
    payload['params']['restarts'] = restarts
    payload['params']['time_limit_ms'] = time_limit
    payload['params']['seed'] = seed
    
    try:
        response = requests.post(API_URL, json=payload, timeout=(time_limit/1000.0) + 5)
        if response.status_code == 200:
            return response.json()
        else:
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
    # stock_width and stock_height are not needed for the new compactness score, 
    # but could be useful for other metrics. Keeping them for context for now.
    # stock_width = template['stock'][0]['width_mm']
    # stock_height = template['stock'][0]['height_mm']
    
    top_candidates = []
    
    log_header = ['restarts', 'time_limit_ms', 'seed', 'waste_percent', 'compactness_score']
    with open(LOG_FILE, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow(log_header)

        print(f"Starting {NUM_TESTS} optimization tests (Mode: GUILLOTINE)...")
        print(f"Target: Find Top {TOP_N_CANDIDATES} candidates by Waste % and Compactness Score.")
        print(f"Logging all runs to {LOG_FILE}")
        print(f"{'Iter':<5} | {'Restarts':<8} | {'Time':<5} | {'Waste%':<7} | {'CompactScore':<14} | {'Status'}")
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
                score = calculate_compactness_score(result['solutions'])
                
                # Log every run
                writer.writerow([restarts, time_limit, seed, waste, score])
                
                # --- Candidate Management ---
                candidate_data = {
                    'restarts': restarts,
                    'time_limit_ms': time_limit,
                    'seed': seed,
                    'waste_percent': waste,
                    'score': score,
                    'svg': result['artifacts']['svg']
                }

                # Add to list and sort
                top_candidates.append(candidate_data)
                # Sort by waste (ascending), then by score (ascending)
                top_candidates.sort(key=lambda c: (c['waste_percent'], c['score']))
                
                # Trim the list if it's too long
                if len(top_candidates) > TOP_N_CANDIDATES:
                    top_candidates.pop() # Removes the worst candidate

                status_str = f"OK (Waste: {waste:.2f}%)"
                print(f"{i:<5} | {restarts:<8} | {time_limit:<5} | {waste:<7.2f} | {score:<14.0f} | {status_str}")
            else:
                 print(f"{i:<5} | {restarts:<8} | {time_limit:<5} | {'-':<7} | {'-':<14} | {status_str}")


    print("\n" + "="*40)
    print("Optimization Search Completed.")
    print("="*40)

    if not top_candidates:
        print("No successful optimization runs found.")
        return

    print(f"\nTop {len(top_candidates)} Candidates (Sorted by Waste, then Compactness Score):")
    print("-" * 80)
    print(f"{'Rank':<5} | {'Waste%':<7} | {'CompactScore':<14} | {'Restarts':<8} | {'Time':<5} | {'Seed'}")
    print("-" * 80)

    for idx, candidate in enumerate(top_candidates):
        rank = idx + 1
        # Save SVG to file
        svg_filename = f"rank_{rank:02d}_waste_{candidate['waste_percent']:.2f}_compactscore_{candidate['score']:.0f}.svg"
        svg_filepath = os.path.join(CANDIDATES_DIR, svg_filename)
        with open(svg_filepath, 'w') as f:
            f.write(candidate['svg'])

        print(f"{rank:<5} | {candidate['waste_percent']:<7.2f} | {candidate['score']:<14.0f} | {candidate['restarts']:<8} | {candidate['time_limit_ms']:<5} | {candidate['seed']}")

    print("-" * 80)
    print(f"\nSaved {len(top_candidates)} candidate SVGs to: {CANDIDATES_DIR}")
    print(f"Full run data logged to: {LOG_FILE}")
    print("\nAnalysis of the relationship between parameters can now be performed on this log file.")

if __name__ == "__main__":
    main()

