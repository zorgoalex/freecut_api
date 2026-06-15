"""V34b quick sweep: profile_pool with mp=0.0 (baseline) vs mp=0.2 vs mp=0.5."""

import json
import os
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "ai_docs", "tmp", "best_layouts_v34b_quick")
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8092")
BASE_URL = f"http://127.0.0.1:{PORT}"
SEEDS = 8
MP_VALUES = [0.0, 0.2, 0.5]
ZONE_PENALTIES = [0.2, 0.3, 0.4, 0.5]

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 8000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = False
base_req["params"]["portfolio"] = {"enabled": True, "candidate_count": 5, "deadline_ms": 8000}
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
base_req["params"]["partition"] = {"enabled": True, "sheet_budget_ms": 15000}


def call_optimize(req):
    data = json.dumps(req).encode("utf-8")
    r = urllib.request.Request(f"{BASE_URL}/v1/optimize", data=data, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        print(f"  HTTP {e.code}", flush=True)
        return None


def sheet_utils(response):
    out = []
    for s in response.get("solutions", []):
        trim = s.get("trim_mm", {})
        w = s["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = s["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(p["width_mm"] * p["height_mm"] for p in s.get("placements", []))
        out.append(used / (w * h) * 100.0 if w > 0 and h > 0 else 0.0)
    return out


def waste_regions(resp):
    pool = resp.get("summary", {}).get("profile_pool") or {}
    wr = pool.get("winner_waste_regions", 0)
    if wr > 0:
        return wr
    total = 0
    for sol in resp.get("solutions", []):
        for ss in sol.get("used_stock", []):
            total += ss.get("waste_regions", 0)
    return total


all_results = {}
for mp in MP_VALUES:
    tag = f"mp{mp}"
    print(f"\n=== {tag} ===", flush=True)
    rows = []
    for seed in range(1, SEEDS + 1):
        req = json.loads(json.dumps(base_req))
        req["params"]["seed"] = seed
        req["params"]["profile_pool"] = {
            "enabled": True,
            "zone_penalties": ZONE_PENALTIES,
            "fill_penalty": 0.1,
            "max_lead_drop_pp": 0.8,
            "monotonicity_penalty": mp,
        }
        t0 = time.time()
        resp = call_optimize(req)
        elapsed = time.time() - t0
        if resp is None:
            print(f"  s{seed}: FAIL", flush=True)
            continue
        utils = sheet_utils(resp)
        ns = len(resp.get("solutions", []))
        lead = sum(sorted(utils, reverse=True)[:-1]) / max(1, len(utils) - 1) if len(utils) > 1 else (utils[0] if utils else 0)
        zones = waste_regions(resp)
        rows.append({"seed": seed, "sheets": ns, "lead_util": round(lead, 2), "zones": zones, "elapsed": round(elapsed, 1)})
        print(f"  s{seed}: {ns}s lead={lead:.2f}% z={zones}", flush=True)

    if rows:
        n4 = sum(1 for r in rows if r["sheets"] == 4)
        avg_lead = sum(r["lead_util"] for r in rows) / len(rows)
        avg_z = sum(r["zones"] for r in rows) / len(rows)
        le4 = sum(1 for r in rows if r["zones"] <= 4)
        le5 = sum(1 for r in rows if r["zones"] <= 5)
        print(f"  SUMMARY: 4s={n4}/{len(rows)} avg_lead={avg_lead:.2f}% avg_zones={avg_z:.1f} le4={le4} le5={le5}", flush=True)
    all_results[tag] = rows

with open(os.path.join(OUT_DIR, "v34b_quick_results.json"), "w") as f:
    json.dump(all_results, f, indent=2)
print(f"\nSaved to {OUT_DIR}", flush=True)