"""Analyze the void structure of nested+portfolio solutions."""
import json, requests, copy

FIXTURE = "tests/fixtures/multisheet_varied_4sheets.json"
URL = "http://localhost:8088/v1/optimize"

with open(FIXTURE) as f:
    base_req = json.load(f)

base_req['params']['seed'] = 0
base_req['params']['layout_mode'] = 'nested'
base_req['params']['time_limit_ms'] = 5000
base_req['params']['restarts'] = 5
base_req['params']['portfolio'] = {'enabled': True, 'candidate_count': 5, 'deadline_ms': 5000}

r = requests.post(URL, json=base_req, timeout=60)
data = r.json()
sols = data.get('solutions', [])

total_void = 0
for i, sol in enumerate(sols):
    pls = sol.get('placements', [])
    w = sol['width_mm']
    h = sol['height_mm']
    sheet_area = w * h
    used = sum(p['width_mm'] * p['height_mm'] for p in pls)
    
    if not pls:
        continue
    min_x = min(p['x_mm'] for p in pls)
    min_y = min(p['y_mm'] for p in pls)
    max_x = max(p['x_mm'] + p['width_mm'] for p in pls)
    max_y = max(p['y_mm'] + p['height_mm'] for p in pls)
    bbox_w = max_x - min_x
    bbox_h = max_y - min_y
    bbox_area = bbox_w * bbox_h
    void = bbox_area - used
    total_void += void
    util = used / bbox_area * 100 if bbox_area > 0 else 0
    
    print(f"\nSheet {i}: {len(pls)} pieces")
    print(f"  Sheet size:  {w} x {h} = {sheet_area:.0f} mm2")
    print(f"  Bbox:        {bbox_w:.0f} x {bbox_h:.0f} = {bbox_area:.0f} mm2")
    print(f"  Used area:   {used:.0f} mm2")
    print(f"  Bbox void:   {void:.0f} mm2 ({void/sheet_area*100:.1f}% of sheet)")
    print(f"  Utilization: {util:.1f}%")
    print(f"  Edge gaps:   L={min_x:.0f} R={w-max_x:.0f} T={min_y:.0f} B={h-max_y:.0f}")
    
    # Sort by y position and show piece layout
    for p in sorted(pls, key=lambda x: (x['y_mm'], x['x_mm'])):
        item = p['item_id']
        inst = p['instance']
        x, y = p['x_mm'], p['y_mm']
        pw, ph = p['width_mm'], p['height_mm']
        rot = 'R' if p['rotated'] else ' '
        print(f"    {item:>12}#{inst} {rot} pos=({x:6.0f},{y:6.0f}) size={pw:5.0f}x{ph:5.0f}  right_edge={x+pw:.0f} bottom={y+ph:.0f}")

print(f"\n{'='*60}")
print(f"Total bbox void across all sheets: {total_void:.0f} mm2 ({total_void/1e6:.2f}M)")
print(f"Average void per sheet: {total_void/len(sols):.0f} mm2")

# Also analyze piece sizes to understand what could fill the voids
print(f"\n{'='*60}")
print(f"Piece inventory:")
total_piece_area = 0
for item in base_req['items']:
    pw = item['width_mm']
    ph = item['height_mm']
    qty = item['qty']
    area = pw * ph
    total_piece_area += area * qty
    print(f"  {item['id']:>12}: {pw:.0f}x{ph:.0f} = {area:.0f} mm2 x{qty} = {area*qty:.0f} mm2")

total_sheet_area = sum(s['width_mm'] * s['height_mm'] for s in sols)
print(f"\nTotal piece area: {total_piece_area:.0f} mm2")
print(f"Total sheet area: {total_sheet_area:.0f} mm2")
print(f"Waste: {total_sheet_area - total_piece_area:.0f} mm2 ({(total_sheet_area - total_piece_area)/total_sheet_area*100:.1f}%)")
print(f"Piece area / (4 sheets): {total_piece_area/(4*2050*2780)*100:.1f}% utilization")
