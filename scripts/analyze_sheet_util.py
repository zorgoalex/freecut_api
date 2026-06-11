"""Detailed per-sheet utilization analysis for top-5 layouts."""
import json, os, glob

DIR = "ai_docs/tmp/best_layouts_frag02"
SHEET_AREA = (2070 - 20) * (2800 - 20)  # usable area after trim

for jf in sorted(glob.glob(os.path.join(DIR, "*.json"))):
    with open(jf) as f:
        data = json.load(f)
    name = os.path.basename(jf).replace(".json", "")
    print(f"\n{'='*70}")
    print(f"{name} (seed={data['seed']}, waste={data['waste']:.0f} mm2)")
    print(f"{'='*70}")
    total_pieces = 0
    total_used = 0
    for sol in data["solutions"]:
        trim = sol.get("trim_mm", {})
        uw = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        uh = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        sheet_a = uw * uh
        pieces = sol.get("placements", [])
        used = sum(p["width_mm"] * p["height_mm"] for p in pieces)
        util = used / sheet_a * 100
        total_pieces += len(pieces)
        total_used += used

        if pieces:
            max_x = max(p["x_mm"] + p["width_mm"] for p in pieces)
            max_y = max(p["y_mm"] + p["height_mm"] for p in pieces)
            bbox_fill = (max_x * max_y) / sheet_a * 100
            # Unused edge space
            edge_x = uw - max_x
            edge_y = uh - max_y
        else:
            max_x = max_y = bbox_fill = edge_x = edge_y = 0

        print(f"  Sheet {sol['index']}: {len(pieces):2d} pcs, util={util:.1f}%, "
              f"bbox_fill={bbox_fill:.1f}%, "
              f"edge_gap: x={edge_x:.0f}mm y={edge_y:.0f}mm")

        # Show pieces sorted by area
        p_sorted = sorted(
            [(p["item_id"], p["width_mm"], p["height_mm"],
              p["x_mm"], p["y_mm"], p["width_mm"] * p["height_mm"])
             for p in pieces],
            key=lambda x: -x[5]
        )
        for pid, pw, ph, px, py, pa in p_sorted:
            tag = ""
            if pa < 200000:
                tag = " [SMALL]"
            elif pa < 500000:
                tag = " [MED]"
            print(f"    {pid:12s} {pw:7.0f}x{ph:7.0f} = {pa:>10.0f} mm2 "
                  f"@ ({px:6.0f},{py:6.0f}){tag}")

    total_util = total_used / (SHEET_AREA * len(data["solutions"])) * 100
    print(f"\n  TOTAL: {total_pieces} pcs, avg_util={total_util:.1f}%, "
          f"total_used={total_used:.0f} / {SHEET_AREA * len(data['solutions']):.0f} mm2")
