#!/usr/bin/env python3
import json
import sys

resp = sys.stdin.read()
try:
    data = json.loads(resp)
except Exception as e:
    print("optimize parse error:", e)
    sys.exit(1)

if data.get("status") != "ok":
    print("optimize status not ok:", data.get("status"))
    sys.exit(1)

solutions = data.get("solutions") or []
if not solutions:
    print("optimize solutions empty")
    sys.exit(1)

svg = data.get("artifacts", {}).get("svg", "")
if not (svg.startswith("<svg") and svg.endswith("</svg>")):
    print("svg missing or invalid")
    sys.exit(1)

fmt = lambda v: f"{v:.3f}"
checked = 0
for sol in solutions:
    for p in sol.get("placements", []):
        checked += 1
        rect = (
            f"<rect x=\"{fmt(p['x_mm'])}\" y=\"{fmt(p['y_mm'])}\" "
            f"width=\"{fmt(p['width_mm'])}\" height=\"{fmt(p['height_mm'])}\" "
            f"fill=\"#cfe8ff\""
        )
        if rect not in svg:
            print("svg missing rect for placement", p)
            sys.exit(1)
        tx = fmt(p["x_mm"] + 2.0)
        ty = fmt(p["y_mm"] + 12.0)
        label = p["item_id"]
        label = (
            label.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace('"', "&quot;")
            .replace("'", "&apos;")
        )
        text = (
            f"<text x=\"{tx}\" y=\"{ty}\" font-size=\"10\" "
            f"fill=\"#1f4a6d\">{label}</text>"
        )
        if text not in svg:
            print("svg missing text for placement", p)
            sys.exit(1)

if checked == 0:
    print("no placements to check")
    sys.exit(1)

print("optimize ok; placements checked:", checked)
