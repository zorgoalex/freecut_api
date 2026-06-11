"""Quick test: generate one layout with improved SVG."""
import json, urllib.request

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

# Check for kerf-fill rectangles
kerf_count = svg.count('fill="#7ab0d4"')
print(f"Kerf-fill rectangles: {kerf_count}")

with open("ai_docs/tmp/test_kerf_fill.svg", "w") as f:
    f.write(svg)
print("Saved to ai_docs/tmp/test_kerf_fill.svg")
