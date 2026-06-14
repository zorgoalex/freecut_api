"""Quick test: generate one layout with improved SVG."""
import json, os, urllib.request

OUT_DIR = "ai_docs/tmp/best_layouts_visible_kerf"
os.makedirs(OUT_DIR, exist_ok=True)

with open("tests/fixtures/multisheet_varied_4sheets.json") as f:
    req = json.load(f)

req["params"]["time_limit_ms"] = 10000
req["params"]["restarts"] = 5
req["params"]["layout_mode"] = "guillotine"
req["params"]["seed"] = 15
req["params"]["include_svg"] = True
req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 10000}

payload = json.dumps(req).encode()
r = urllib.request.Request("http://localhost:8088/v1/optimize", data=payload,
    headers={"Content-Type": "application/json"})
resp = urllib.request.urlopen(r, timeout=120)
data = json.loads(resp.read())

svg = data.get("artifacts", {}).get("svg", "") or ""
sols = data.get("solutions", [])
print(f"Sheets: {len(sols)}, SVG length: {len(svg)}")

# Check for kerf-fill rectangles (now dark red #8B0000)
kerf_count = svg.count('fill="#8B0000"')
print(f"Kerf-fill rectangles: {kerf_count}")

svg_path = os.path.join(OUT_DIR, "test_kerf_fill_seed15.svg")
with open(svg_path, "w") as f:
    f.write(svg)
print(f"Saved to {svg_path}")

# also save json summary (no svg)
summary = {k: v for k, v in data.items() if k != "artifacts" or True}
if "artifacts" in summary and "svg" in summary["artifacts"]:
    summary["artifacts"] = {k: v for k, v in summary["artifacts"].items() if k != "svg"}
with open(os.path.join(OUT_DIR, "test_kerf_fill_seed15.json"), "w") as f:
    json.dump(summary, f, indent=2)
