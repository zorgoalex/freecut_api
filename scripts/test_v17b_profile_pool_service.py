"""V17b hypothesis test: built-in service profile_pool.

Calls `/v1/optimize` once per seed with `params.profile_pool` enabled.  The
service runs the configured zone-penalty profiles internally and returns the
winner plus `summary.profile_pool` telemetry.
"""

import json
import os
import sys
import time
import urllib.request

try:
    from PIL import Image, ImageDraw
except Exception:
    Image = None
    ImageDraw = None

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.environ.get(
    "FREECUT_OUT_DIR",
    os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v17b_profile_pool_service"),
)
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8088")
SEEDS = int(os.environ.get("FREECUT_SEEDS", "8"))
TIME_LIMIT_MS = int(os.environ.get("FREECUT_TIME_LIMIT_MS", "10000"))
SHEET_BUDGET_MS = int(os.environ.get("FREECUT_SHEET_BUDGET_MS", "20000"))
PROFILES = [
    float(x.strip())
    for x in os.environ.get("FREECUT_PROFILE_POOL", "0.3,0.5").split(",")
    if x.strip()
]
MAX_LEAD_DROP_PP = float(os.environ.get("FREECUT_PROFILE_POOL_MAX_LEAD_DROP_PP", "0.4"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = TIME_LIMIT_MS
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = True
base_req["params"]["portfolio"] = {
    "enabled": True,
    "candidate_count": 5,
    "deadline_ms": TIME_LIMIT_MS,
}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
base_req["params"]["partition"] = {"enabled": True}
if SHEET_BUDGET_MS:
    base_req["params"]["partition"]["sheet_budget_ms"] = SHEET_BUDGET_MS
base_req["params"]["profile_pool"] = {
    "enabled": True,
    "zone_penalties": PROFILES,
    "fill_penalty": 0.1,
    "max_lead_drop_pp": MAX_LEAD_DROP_PP,
}


def call_optimize(seed):
    req = json.loads(json.dumps(base_req))
    req["params"]["seed"] = seed
    payload = json.dumps(req).encode()
    request = urllib.request.Request(
        f"http://localhost:{PORT}/v1/optimize",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read())


def sheet_utils(response):
    out = []
    for solution in response.get("solutions", []):
        trim = solution.get("trim_mm", {})
        width = solution["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        height = solution["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(
            p["width_mm"] * p["height_mm"]
            for p in solution.get("placements", [])
        )
        out.append(used / (width * height) * 100.0 if width > 0 and height > 0 else 0.0)
    return out


def row_from_response(seed, data):
    pool = data.get("summary", {}).get("profile_pool") or {}
    utils = sheet_utils(data)
    return {
        "seed": seed,
        "sheets": len(data.get("solutions", [])),
        "zone_penalty": pool.get("winner_zone_penalty"),
        "n_waste_regions": pool.get("winner_waste_regions", 0),
        "lead_util": round(pool.get("winner_lead_util_pct", 0.0), 2),
        "max_corner_mm2": round(pool.get("winner_max_corner_mm2", 0.0)),
        "utils": [round(u, 1) for u in utils],
        "candidates_completed": pool.get("candidates_completed", 0),
        "candidates_timed_out": pool.get("candidates_timed_out", 0),
        "data": data,
    }


def color_for(value):
    state = 2166136261
    for ch in value:
        state ^= ord(ch)
        state = (state * 16777619) & 0xFFFFFFFF
    return (
        80 + (state & 0x7F),
        80 + ((state >> 8) & 0x7F),
        80 + ((state >> 16) & 0x7F),
    )


def save_contact_sheet(rows, out_dir):
    if Image is None:
        print("Pillow is unavailable; contact sheet skipped", flush=True)
        return
    if not rows:
        return

    sheet_w = 320
    sheet_h = 170
    label_h = 38
    gap = 14
    left_label_w = 210
    max_sheets = max(len(row.get("data", {}).get("solutions", [])) for row in rows)
    width = left_label_w + max_sheets * sheet_w + (max_sheets + 1) * gap
    height = label_h + len(rows) * (sheet_h + gap) + gap
    img = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(img)

    draw.text((gap, 10), "V17c profile_pool top layouts", fill=(20, 20, 20))
    for row_idx, row in enumerate(rows):
        y0 = label_h + row_idx * (sheet_h + gap)
        label = (
            f"rank {row_idx + 1}  seed {row['seed']}  "
            f"zp {row['zone_penalty']:.2f}  zones {row['n_waste_regions']}  "
            f"lead {row['lead_util']:.2f}%"
        )
        draw.text((gap, y0 + 6), label, fill=(20, 20, 20))
        solutions = row.get("data", {}).get("solutions", [])
        for sheet_idx, solution in enumerate(solutions):
            x0 = left_label_w + gap + sheet_idx * (sheet_w + gap)
            trim = solution.get("trim_mm", {})
            usable_w = solution["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
            usable_h = solution["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
            scale = min((sheet_w - 8) / usable_w, (sheet_h - 24) / usable_h)
            ox = x0 + 4
            oy = y0 + 20
            bw = usable_w * scale
            bh = usable_h * scale
            draw.rectangle([ox, oy, ox + bw, oy + bh], outline=(30, 30, 30), fill=(246, 246, 246))
            for placement in solution.get("placements", []):
                px = ox + placement["x_mm"] * scale
                py = oy + placement["y_mm"] * scale
                pw = placement["width_mm"] * scale
                ph = placement["height_mm"] * scale
                fill = color_for(placement.get("item_id", "item"))
                draw.rectangle([px, py, px + pw, py + ph], fill=fill, outline=(255, 255, 255))
            util = row.get("utils", [])
            util_text = f"S{sheet_idx + 1}"
            if sheet_idx < len(util):
                util_text += f" {util[sheet_idx]:.1f}%"
            draw.text((x0 + 4, y0 + 3), util_text, fill=(40, 40, 40))

    path = os.path.join(out_dir, "layout_contact_sheet.png")
    img.save(path)
    print(f"Saved contact sheet to {path}", flush=True)


def main():
    rows = []
    started = time.time()
    print(f"profiles={PROFILES}, max_lead_drop_pp={MAX_LEAD_DROP_PP}", flush=True)
    for seed in range(1, SEEDS + 1):
        try:
            data = call_optimize(seed)
        except Exception as exc:
            print(f"  seed={seed:2d}: ERROR {exc}", flush=True)
            continue
        row = row_from_response(seed, data)
        rows.append(row)
        print(
            f"  seed={seed:2d}: sheets={row['sheets']}, zp={row['zone_penalty']}, "
            f"lead={row['lead_util']:5.2f}%, regions={row['n_waste_regions']}, "
            f"corner={row['max_corner_mm2'] / 1e3:.0f}k, "
            f"completed={row['candidates_completed']}, timeouts={row['candidates_timed_out']}",
            flush=True,
        )

    n = max(1, len(rows))
    n_4 = sum(1 for r in rows if r["sheets"] == 4)
    avg_lead = sum(r["lead_util"] for r in rows) / n
    avg_regions = sum(r["n_waste_regions"] for r in rows) / n
    avg_max_corner = sum(r["max_corner_mm2"] for r in rows) / n
    n_regions_le_4 = sum(1 for r in rows if r["n_waste_regions"] <= 4)
    n_regions_le_5 = sum(1 for r in rows if r["n_waste_regions"] <= 5)

    print(f"\nV17b Service Profile Pool Summary ({len(rows)} seeds): elapsed {time.time() - started:.0f}s", flush=True)
    print(f"  4-sheet rate:                 {n_4}/{len(rows)}", flush=True)
    print(f"  Avg lead util (best n-1):     {avg_lead:.2f}%", flush=True)
    print(f"  Avg waste regions per layout: {avg_regions:.2f}", flush=True)
    print(f"  Avg max corner rect:          {avg_max_corner / 1e3:.0f}k mm2", flush=True)
    print(f"  Layouts with <=4 regions:     {n_regions_le_4}/{len(rows)}", flush=True)
    print(f"  Layouts with <=5 regions:     {n_regions_le_5}/{len(rows)}", flush=True)

    ranked = sorted(rows, key=lambda r: (r["sheets"] != 4, r["n_waste_regions"], -r["lead_util"]))
    for i, row in enumerate(ranked[:5], start=1):
        svg = row.get("data", {}).get("artifacts", {}).get("svg", "")
        stem = f"rank_{i:02d}_zones{row['n_waste_regions']}_seed_{row['seed']}_zp{row['zone_penalty']:.2f}"
        if svg:
            with open(os.path.join(OUT_DIR, stem + ".svg"), "w", encoding="utf-8") as f:
                f.write(svg)
        with open(os.path.join(OUT_DIR, stem + ".json"), "w", encoding="utf-8") as f:
            json.dump({k: v for k, v in row.items() if k != "data"}, f, indent=2, ensure_ascii=False)
        print(
            f"  rank {i}: seed={row['seed']}, zp={row['zone_penalty']:.2f}, "
            f"zones={row['n_waste_regions']}, lead={row['lead_util']}%, utils={row['utils']}",
            flush=True,
        )
    save_contact_sheet(ranked[:5], OUT_DIR)

    with open(os.path.join(OUT_DIR, "v17b_profile_pool_service_summary.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "profiles": PROFILES,
                "max_lead_drop_pp": MAX_LEAD_DROP_PP,
                "results": [{k: v for k, v in r.items() if k != "data"} for r in rows],
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"Saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
